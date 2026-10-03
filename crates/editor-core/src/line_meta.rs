//! Метаданные строк документа: закладки и folded-состояния.
//!
//! Индекс — order-statistic treap (арена из `Vec<Node>`, max-heap по
//! случайному приоритету, детерминированный splitmix64) над строками
//! документа: в узле флаги `bookmark`/`folded` и суммы по поддеревьям.
//! Точечная установка флага, вставка/удаление диапазона строк и запросы
//! агрегатов (`bookmark_count`, `next/prev_bookmark`) — O(log n);
//! `bookmarked_lines()` — O(n) обход (нужен целиком для панели закладок).
//!
//! Индекс живёт внутри `Document` и синхронизируется в `Document::apply`
//! без полного пересчёта: для каждой `Edit` число строк меняется на
//! `('\n' во вставленном) - ('\n' в удаляемом)`. Строка, содержащая
//! `edit.start`, сохраняет флаги; строки `(L, L+k]` исчезают вместе с
//! флагами; вставленные строки — свежие, без флагов, остальные
//! сдвигаются. Char↔line не дублируется — ropey даёт O(log n)
//! `char_to_line`/`line_to_char`; treap хранит только метаданные.

use ropey::Rope;

use crate::transaction::Transaction;

const NIL: u32 = u32::MAX;

#[derive(Clone, Debug)]
struct Node {
    left: u32,
    right: u32,
    /// Приоритет max-heap — случайность балансирует treap.
    prio: u64,
    /// Число строк в поддереве.
    size: u32,
    /// Суммы флагов по поддереву.
    bookmarks: u32,
    folds: u32,
    bookmark: bool,
    folded: bool,
}

/// Индекс метаданных строк: флаги `bookmark` и `folded` на каждую строку
/// плюс агрегаты для навигации по закладкам (F2/Shift+F2) и рендера
/// folding. Номера строк — 0-based, синхронны с `Document::len_lines()`.
#[derive(Debug)]
pub struct LineMeta {
    nodes: Vec<Node>,
    root: u32,
    rng: u64,
}

impl Default for LineMeta {
    fn default() -> Self {
        Self::new()
    }
}

impl LineMeta {
    pub fn new() -> Self {
        LineMeta { nodes: Vec::new(), root: NIL, rng: 0x853C_49E6_748F_EA9B }
    }

    /// Пустой индекс на `line_count` строк (все флаги сброшены).
    pub fn with_lines(line_count: usize) -> Self {
        let mut meta = Self::new();
        meta.build_balanced(line_count);
        meta
    }

    /// Число строк под индексом (инвариант: совпадает с
    /// `Document::len_lines()` после каждого `apply`).
    pub fn line_count(&self) -> usize {
        self.size(self.root)
    }

    pub fn bookmark_count(&self) -> usize {
        self.bm_sum(self.root) as usize
    }

    pub fn folded_count(&self) -> usize {
        self.fd_sum(self.root) as usize
    }

    pub fn set_bookmark(&mut self, line: usize) {
        self.set_flags(line, Some(true), None);
    }

    pub fn clear_bookmark(&mut self, line: usize) {
        self.set_flags(line, Some(false), None);
    }

    pub fn toggle_bookmark(&mut self, line: usize) {
        if self.is_bookmarked(line) {
            self.clear_bookmark(line);
        } else {
            self.set_bookmark(line);
        }
    }

    pub fn is_bookmarked(&self, line: usize) -> bool {
        self.flags(line).0
    }

    /// Все строки с закладками, по возрастанию.
    pub fn bookmarked_lines(&self) -> Vec<usize> {
        let mut out = Vec::new();
        self.collect(self.root, 0, false, &mut out);
        out
    }

    /// Следующая закладка после строки `line` (строго больше);
    /// если её нет — циклически с начала файла. `None` — закладок нет.
    pub fn next_bookmark(&self, line: usize) -> Option<usize> {
        self.first_bookmarked(self.root, 0, line + 1)
            .or_else(|| self.first_bookmarked(self.root, 0, 0))
    }

    /// Предыдущая закладка до строки `line` (строго меньше);
    /// если её нет — циклически с конца файла. `None` — закладок нет.
    pub fn prev_bookmark(&self, line: usize) -> Option<usize> {
        self.last_bookmarked(self.root, 0, line)
            .or_else(|| self.last_bookmarked(self.root, 0, usize::MAX))
    }

    pub fn set_folded(&mut self, line: usize, folded: bool) {
        self.set_flags(line, None, Some(folded));
    }

    pub fn toggle_folded(&mut self, line: usize) {
        self.set_folded(line, !self.is_folded(line));
    }

    pub fn is_folded(&self, line: usize) -> bool {
        self.flags(line).1
    }

    /// Все свёрнутые строки, по возрастанию.
    pub fn folded_lines(&self) -> Vec<usize> {
        let mut out = Vec::new();
        self.collect(self.root, 0, true, &mut out);
        out
    }

    /// Синхронизация с транзакцией по до-транзакционному состоянию rope.
    /// Вызывается из `Document::apply` ДО мутации rope: правки не
    /// пересекаются и отсортированы, поэтому `char_to_line(edit.start)`
    /// и состав удаляемого текста валидны для всех правок сразу.
    ///
    /// Правило привязки флагов (к содержимому строки): если `edit.start`
    /// стоит на границе строки `L` (удаление/вставка целых строк), строки
    /// `L..L+k` исчезают вместе с флагами, а флаги строки `L` при вставке
    /// уезжают вниз за её содержимым; если `edit.start` внутри строки —
    /// строка `L` сохраняет флаги (её содержимое частично живо), строки
    /// `L+1..L+k` уничтожаются.
    pub fn apply_transaction(&mut self, tx: &Transaction, rope: &Rope) {
        let edits = tx.edits();
        if edits.is_empty() {
            return;
        }

        let mut start_lines = Vec::with_capacity(edits.len());
        let mut at_boundary = Vec::with_capacity(edits.len());
        let mut removed_newlines = Vec::with_capacity(edits.len());
        for edit in edits {
            let line = rope.char_to_line(edit.start);
            start_lines.push(line);
            at_boundary.push(edit.start == rope.line_to_char(line));
            removed_newlines.push(
                if edit.end > edit.start {
                    rope.slice(edit.start..edit.end).chars().filter(|&c| c == '\n').count()
                } else {
                    0
                },
            );
        }

        // Правки по возрастанию: строка каждой правки в текущих
        // координатах = до-транзакционная строка + кумулятивная дельта
        // предыдущих правок (вставлено - удалено строк).
        let mut delta: i64 = 0;
        for (i, edit) in edits.iter().enumerate() {
            let line = (start_lines[i] as i64 + delta).max(0) as usize;
            let removed = removed_newlines[i];
            let inserted = edit.text.chars().filter(|&c| c == '\n').count();
            // На границе строки диапазон удаляемых/вставляемых строк
            // начинается с самой строки `line`, внутри строки — со следующей.
            let edge = if at_boundary[i] { line } else { line + 1 };
            if removed > 0 {
                self.remove_lines(edge, removed);
            }
            if inserted > 0 {
                self.insert_lines(edge, inserted);
            }
            delta += inserted as i64 - removed as i64;
        }
    }

    fn size(&self, t: u32) -> usize {
        if t == NIL { 0 } else { self.nodes[t as usize].size as usize }
    }

    /// Прямое O(n) построение начального индекса (D11.2): старая сборка
    /// `insert_lines(0, n)` делала n одиночных `merge` по правой
    /// шпиле — ~0.2 мкс/строку (572 мс и двойной transient при
    /// реаллокациях на 9.4М строк). Здесь строится сбалансированное
    /// по midpoint-поддерево за один проход с точной резервацией арены.
    ///
    /// Приоритеты: старшие 8 бит монотонны по глубине (255 − depth),
    /// младшие 56 бит — из того же детерминированного splitmix64-потока:
    /// max-heap по построению (родитель строго мельче по глубине →
    /// приоритет строго больше), случайность внутри уровня сохраняется.
    /// Дальнейшие вставки/удаления со «случайными» приоритетами
    /// цепляются к скелету — обычные инварианты treap выполняются.
    fn build_balanced(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.nodes.reserve_exact(count);
        /// Рекурсия глубиной log2(count) ≤ 64: [lo, hi) — диапазон строк.
        fn build(meta: &mut LineMeta, lo: usize, hi: usize, depth: u32) -> u32 {
            if lo >= hi {
                return NIL;
            }
            let mid = lo + (hi - lo) / 2;
            let left = build(meta, lo, mid, depth + 1);
            let right = build(meta, mid + 1, hi, depth + 1);
            let prio_high = (255u64.saturating_sub(u64::from(depth))) << 56;
            let prio = prio_high | (meta.next_prio() & 0x00FF_FFFF_FFFF_FFFF);
            meta.nodes.push(Node {
                left,
                right,
                prio,
                size: 0,
                bookmarks: 0,
                folds: 0,
                bookmark: false,
                folded: false,
            });
            let t = (meta.nodes.len() - 1) as u32;
            meta.pull(t);
            t
        }
        self.root = build(self, 0, count, 0);
    }

    fn bm_sum(&self, t: u32) -> u32 {
        if t == NIL { 0 } else { self.nodes[t as usize].bookmarks }
    }

    fn fd_sum(&self, t: u32) -> u32 {
        if t == NIL { 0 } else { self.nodes[t as usize].folds }
    }

    /// Пересчёт агрегатов узла из детей.
    fn pull(&mut self, t: u32) {
        let (left, right, bookmark, folded) = {
            let n = &self.nodes[t as usize];
            (n.left, n.right, n.bookmark, n.folded)
        };
        let size = self.size(left) + self.size(right) + 1;
        let bookmarks = self.bm_sum(left) + self.bm_sum(right) + u32::from(bookmark);
        let folds = self.fd_sum(left) + self.fd_sum(right) + u32::from(folded);
        let n = &mut self.nodes[t as usize];
        n.size = size as u32;
        n.bookmarks = bookmarks;
        n.folds = folds;
    }

    fn next_prio(&mut self) -> u64 {
        self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn new_node(&mut self, bookmark: bool, folded: bool) -> u32 {
        let prio = self.next_prio();
        self.nodes.push(Node {
            left: NIL,
            right: NIL,
            prio,
            size: 1,
            bookmarks: u32::from(bookmark),
            folds: u32::from(folded),
            bookmark,
            folded,
        });
        (self.nodes.len() - 1) as u32
    }

    /// Разделяет поддерево: первые `k` строк — в левую часть, остальные —
    /// в правую.
    fn split(&mut self, t: u32, k: usize) -> (u32, u32) {
        if t == NIL {
            return (NIL, NIL);
        }
        let (left, right, lsize) = {
            let n = &self.nodes[t as usize];
            (n.left, n.right, self.size(n.left))
        };
        if k <= lsize {
            let (a, b) = self.split(left, k);
            self.nodes[t as usize].left = b;
            self.pull(t);
            (a, t)
        } else {
            let (a, b) = self.split(right, k - lsize - 1);
            self.nodes[t as usize].right = a;
            self.pull(t);
            (t, b)
        }
    }

    fn merge(&mut self, a: u32, b: u32) -> u32 {
        if a == NIL {
            return b;
        }
        if b == NIL {
            return a;
        }
        let (a_prio, a_right, b_prio, b_left) = {
            let na = &self.nodes[a as usize];
            let nb = &self.nodes[b as usize];
            (na.prio, na.right, nb.prio, nb.left)
        };
        if a_prio > b_prio {
            let m = self.merge(a_right, b);
            self.nodes[a as usize].right = m;
            self.pull(a);
            a
        } else {
            let m = self.merge(a, b_left);
            self.nodes[b as usize].left = m;
            self.pull(b);
            b
        }
    }

    /// Вставляет `count` свежих строк (без флагов) перед строкой `pos.
    fn insert_lines(&mut self, pos: usize, count: usize) {
        if count == 0 {
            return;
        }
        debug_assert!(pos <= self.line_count(), "insert_lines: pos {pos} > {}", self.line_count());
        let (a, b) = self.split(self.root, pos);
        let mut fresh = NIL;
        for _ in 0..count {
            let n = self.new_node(false, false);
            fresh = self.merge(fresh, n);
        }
        let left = self.merge(a, fresh);
        self.root = self.merge(left, b);
    }

    /// Удаляет строки `[pos, pos + count)` вместе с их флагами.
    fn remove_lines(&mut self, pos: usize, count: usize) {
        if count == 0 {
            return;
        }
        let total = self.line_count();
        debug_assert!(pos <= total, "remove_lines: pos {pos} > {total}");
        let count = count.min(total.saturating_sub(pos));
        if count == 0 {
            return;
        }
        let (a, bc) = self.split(self.root, pos);
        let (_, c) = self.split(bc, count);
        self.root = self.merge(a, c);
    }

    fn set_flags(&mut self, line: usize, bookmark: Option<bool>, folded: Option<bool>) {
        assert!(line < self.line_count(), "line {line} вне диапазона ({} строк)", self.line_count());
        let mut t = self.root;
        let mut path: Vec<u32> = Vec::with_capacity(64);
        let mut idx = line;
        loop {
            let (left, right, lsize) = {
                let n = &self.nodes[t as usize];
                (n.left, n.right, self.size(n.left))
            };
            path.push(t);
            if idx < lsize {
                t = left;
            } else if idx == lsize {
                let n = &mut self.nodes[t as usize];
                if let Some(b) = bookmark {
                    n.bookmark = b;
                }
                if let Some(f) = folded {
                    n.folded = f;
                }
                break;
            } else {
                idx -= lsize + 1;
                t = right;
            }
        }
        for &p in path.iter().rev() {
            self.pull(p);
        }
    }

    fn flags(&self, line: usize) -> (bool, bool) {
        assert!(line < self.line_count(), "line {line} вне диапазона ({} строк)", self.line_count());
        let mut t = self.root;
        let mut idx = line;
        loop {
            let n = &self.nodes[t as usize];
            let lsize = self.size(n.left);
            if idx < lsize {
                t = n.left;
            } else if idx == lsize {
                return (n.bookmark, n.folded);
            } else {
                idx -= lsize + 1;
                t = n.right;
            }
        }
    }

    fn collect(&self, t: u32, base: usize, folded: bool, out: &mut Vec<usize>) {
        if t == NIL {
            return;
        }
        let n = &self.nodes[t as usize];
        let has = if folded { n.folds } else { n.bookmarks };
        if has == 0 {
            return;
        }
        let lsize = self.size(n.left);
        let here = base + lsize;
        self.collect(n.left, base, folded, out);
        if if folded { n.folded } else { n.bookmark } {
            out.push(here);
        }
        self.collect(n.right, here + 1, folded, out);
    }

    /// Минимальная строка >= `target` с закладкой; `base` — номер первой
    /// строки поддерева `t`.
    fn first_bookmarked(&self, t: u32, base: usize, target: usize) -> Option<usize> {
        if t == NIL || self.nodes[t as usize].bookmarks == 0 {
            return None;
        }
        let n = &self.nodes[t as usize];
        let lsize = self.size(n.left);
        // Левое поддерево ищем, только если в нём есть строки >= target.
        if base + lsize > target {
            if let Some(found) = self.first_bookmarked(n.left, base, target) {
                return Some(found);
            }
        }
        let here = base + lsize;
        if here >= target && n.bookmark {
            return Some(here);
        }
        self.first_bookmarked(n.right, here + 1, target)
    }

    /// Максимальная строка < `target` с закладкой.
    fn last_bookmarked(&self, t: u32, base: usize, target: usize) -> Option<usize> {
        if t == NIL || self.nodes[t as usize].bookmarks == 0 {
            return None;
        }
        let n = &self.nodes[t as usize];
        let lsize = self.size(n.left);
        let here = base + lsize;
        // Правое поддерево ищем, только если в нём есть строки < target.
        if here + 1 < target {
            if let Some(found) = self.last_bookmarked(n.right, here + 1, target) {
                return Some(found);
            }
        }
        if here < target && n.bookmark {
            return Some(here);
        }
        self.last_bookmarked(n.left, base, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::transaction::Edit;

    /// Обязательный тест задачи 2.3: закладки переживают вставку
    /// двух строк выше — смещаются вниз на 2, количество не меняется.
    #[test]
    fn bookmarks_survive_insertion_of_two_lines_above() {
        let mut doc = Document::from_str("l0\nl1\nl2\nl3\nl4\n");
        {
            let meta = doc.line_meta_mut();
            meta.set_bookmark(2);
            meta.set_bookmark(4);
        }
        assert_eq!(doc.line_meta().bookmarked_lines(), vec![2, 4]);

        doc.apply(&Transaction::from_edits(vec![Edit::insert(0, "x\ny\n")]));

        let meta = doc.line_meta();
        assert_eq!(doc.text(), "x\ny\nl0\nl1\nl2\nl3\nl4\n");
        assert_eq!(meta.bookmarked_lines(), vec![4, 6]);
        assert_eq!(meta.bookmark_count(), 2);
        assert_eq!(meta.line_count(), doc.len_lines());
    }

    #[test]
    fn insert_below_bookmark_leaves_it_in_place() {
        let mut doc = Document::from_str("l0\nl1\nl2\n");
        doc.line_meta_mut().set_bookmark(0);
        doc.apply(&Transaction::from_edits(vec![Edit::insert(6, "new\n")]));
        assert_eq!(doc.text(), "l0\nl1\nnew\nl2\n");
        assert_eq!(doc.line_meta().bookmarked_lines(), vec![0]);
    }

    #[test]
    fn delete_line_above_shifts_bookmark_up() {
        let mut doc = Document::from_str("a\nb\nc\nd\n");
        doc.line_meta_mut().set_bookmark(2);
        doc.apply(&Transaction::from_edits(vec![Edit::delete(0, 2)]));
        assert_eq!(doc.text(), "b\nc\nd\n");
        assert_eq!(doc.line_meta().bookmarked_lines(), vec![1]);
        assert_eq!(doc.line_meta().line_count(), doc.len_lines());
    }

    #[test]
    fn delete_line_below_keeps_bookmark_in_place() {
        let mut doc = Document::from_str("a\nb\nc\nd\n");
        doc.line_meta_mut().set_bookmark(1);
        // Удаляем строку 3 ("d\n", [6..8)).
        doc.apply(&Transaction::from_edits(vec![Edit::delete(6, 8)]));
        assert_eq!(doc.text(), "a\nb\nc\n");
        assert_eq!(doc.line_meta().bookmarked_lines(), vec![1]);
    }

    /// Флаги строк, чьё содержимое попало под удаление, исчезают вместе
    /// со строками; строки ниже, не затронутые удалением, сохраняют
    /// флаги и сдвигаются вверх.
    #[test]
    fn flags_on_deleted_lines_are_lost() {
        let mut doc = Document::from_str("a\nb\nc\nd\n");
        {
            let meta = doc.line_meta_mut();
            meta.set_bookmark(2);
            meta.set_folded(3, true);
        }
        // Удаляем "b\nc\n" ([2..6)): строки 1 и 2 уничтожаются,
        // строка "d" (fold) не затронута и сдвигается 3→1.
        doc.apply(&Transaction::from_edits(vec![Edit::delete(2, 6)]));
        assert_eq!(doc.text(), "a\nd\n");
        let meta = doc.line_meta();
        assert_eq!(meta.line_count(), 3);
        assert_eq!(meta.bookmark_count(), 0);
        assert_eq!(meta.folded_count(), 1);
        assert_eq!(meta.bookmarked_lines(), Vec::<usize>::new());
        assert_eq!(meta.folded_lines(), vec![1]);
    }

    /// Строка, содержащая edit.start, сохраняет флаги при схлопывании
    /// строк заменой.
    #[test]
    fn replace_edit_keeps_first_line_flags() {
        let mut doc = Document::from_str("a\nb\nc\nd\n");
        {
            let meta = doc.line_meta_mut();
            meta.set_folded(0, true);
            meta.set_bookmark(2);
        }
        // Заменяем "b\nc\n" ([2..6)) на "X": строки 1-2 схлопываются.
        doc.apply(&Transaction::from_edits(vec![Edit { start: 2, end: 6, text: "X".into() }]));
        assert_eq!(doc.text(), "a\nXd\n");
        let meta = doc.line_meta();
        assert_eq!(meta.line_count(), 3);
        assert!(meta.is_folded(0));
        assert_eq!(meta.bookmarked_lines(), Vec::<usize>::new());
    }

    #[test]
    fn multiline_transaction_from_edits_syncs_meta() {
        let mut doc = Document::from_str("one\ntwo\nthree\nfour\n");
        {
            let meta = doc.line_meta_mut();
            meta.set_bookmark(1);
            meta.set_folded(3, true);
        }
        // Позиции правок — до-транзакционные: len_chars = 19.
        let tx = Transaction::from_edits(vec![Edit::delete(0, 4), Edit::insert(19, "x\ny\n")]);
        doc.apply(&tx);
        assert_eq!(doc.text(), "two\nthree\nfour\nx\ny\n");
        let meta = doc.line_meta();
        // Закладка "two" сместилась 1→0; fold "four" 3→2.
        assert_eq!(meta.bookmarked_lines(), vec![0]);
        assert_eq!(meta.folded_lines(), vec![2]);
        assert_eq!(meta.line_count(), doc.len_lines());
    }

    /// D11.2 (O(n)-построение скелета): индексы разных размеров —
    /// счётчики, флаги, агрегаты и синхронизация с транзакциями.
    #[test]
    fn balanced_skeleton_supports_full_api() {
        for n in [0usize, 1, 2, 3, 7, 100, 10_000] {
            let meta = LineMeta::with_lines(n);
            assert_eq!(meta.line_count(), n, "line_count({n})");
            assert_eq!(meta.bookmark_count(), 0);
            assert_eq!(meta.folded_count(), 0);
            assert_eq!(meta.bookmarked_lines(), Vec::<usize>::new());
            assert_eq!(meta.folded_lines(), Vec::<usize>::new());
        }
        // Флаги на скелете: точечные операции и агрегаты.
        let mut meta = LineMeta::with_lines(10_000);
        meta.set_bookmark(0);
        meta.set_bookmark(5_001);
        meta.set_folded(9_999, true);
        assert_eq!(meta.bookmarked_lines(), vec![0, 5_001]);
        assert_eq!(meta.folded_lines(), vec![9_999]);
        assert_eq!(meta.next_bookmark(0), Some(5_001));
        assert_eq!(meta.prev_bookmark(0), Some(5_001), "цикл через конец");
        assert_eq!(meta.line_count(), 10_000);

        // Синхронизация с транзакциями поверх скелета: вставка строк
        // выше сдвигает флаги за содержимым.
        let mut doc = Document::from_str(&"l\n".repeat(4_999)); // 5_000 строк
        doc.line_meta_mut().set_bookmark(4_999);
        doc.apply(&Transaction::from_edits(vec![Edit::insert(0, "x\ny\n")]));
        assert_eq!(doc.line_meta().bookmarked_lines(), vec![5_001]);
        assert_eq!(doc.line_meta().line_count(), doc.len_lines());
    }

    #[test]
    fn toggle_and_clear_bookmark() {
        let mut meta = LineMeta::with_lines(3);
        assert_eq!(meta.bookmark_count(), 0);
        meta.toggle_bookmark(1);
        assert_eq!(meta.bookmarked_lines(), vec![1]);
        assert!(meta.is_bookmarked(1));
        assert!(!meta.is_bookmarked(0));
        meta.toggle_bookmark(1);
        assert_eq!(meta.bookmark_count(), 0);
        meta.set_bookmark(0);
        meta.set_bookmark(0); // идемпотентно
        assert_eq!(meta.bookmarked_lines(), vec![0]);
        meta.clear_bookmark(0);
        assert_eq!(meta.bookmark_count(), 0);
        assert!(!meta.is_bookmarked(0));
    }

    #[test]
    fn folded_flags_and_counts() {
        let mut meta = LineMeta::with_lines(5);
        meta.set_folded(1, true);
        meta.set_folded(3, true);
        assert_eq!(meta.folded_count(), 2);
        assert_eq!(meta.folded_lines(), vec![1, 3]);
        assert!(meta.is_folded(1));
        assert!(!meta.is_folded(2));
        meta.set_folded(1, false);
        assert_eq!(meta.folded_count(), 1);
        meta.toggle_folded(3);
        assert_eq!(meta.folded_count(), 0);
    }

    /// Циклическая навигация F2/Shift+F2: wrap через конец/начало файла.
    #[test]
    fn cyclic_bookmark_navigation() {
        let mut meta = LineMeta::with_lines(6);
        assert_eq!(meta.next_bookmark(0), None);
        assert_eq!(meta.prev_bookmark(3), None);
        meta.set_bookmark(1);
        meta.set_bookmark(4);
        assert_eq!(meta.next_bookmark(0), Some(1));
        assert_eq!(meta.next_bookmark(1), Some(4));
        assert_eq!(meta.next_bookmark(4), Some(1)); // цикл через конец
        assert_eq!(meta.next_bookmark(5), Some(1));
        assert_eq!(meta.prev_bookmark(4), Some(1));
        assert_eq!(meta.prev_bookmark(1), Some(4)); // цикл через начало
        assert_eq!(meta.prev_bookmark(0), Some(4));
        assert_eq!(meta.prev_bookmark(2), Some(1));
    }

    #[test]
    fn insert_at_line_boundaries() {
        // Вставка в конец документа без '\n' не добавляет строк.
        let mut doc = Document::from_str("ab\ncd");
        doc.line_meta_mut().set_bookmark(1);
        doc.apply(&Transaction::from_edits(vec![Edit::insert(5, "tail")]));
        assert_eq!(doc.text(), "ab\ncdtail");
        assert_eq!(doc.line_meta().line_count(), 2);
        assert_eq!(doc.line_meta().bookmarked_lines(), vec![1]);

        // Вставка '\n' в середину строки 0 добавляет одну строку,
        // "cd" смещается вниз.
        doc.apply(&Transaction::from_edits(vec![Edit::insert(1, "\n")]));
        assert_eq!(doc.text(), "a\nb\ncdtail");
        assert_eq!(doc.line_meta().line_count(), 3);
        assert_eq!(doc.line_meta().bookmarked_lines(), vec![2]);

        // Вставка точно на границе строки (начало строки 2).
        let pos = doc.line_to_char(2);
        doc.apply(&Transaction::from_edits(vec![Edit::insert(pos, "\n")]));
        assert_eq!(doc.text(), "a\nb\n\ncdtail");
        assert_eq!(doc.line_meta().line_count(), 4);
        assert_eq!(doc.line_meta().bookmarked_lines(), vec![3]);
    }

    /// Fold-счётчик при сдвигах: сохраняется при вставке строк выше,
    /// обнуляется при удалении свёрнутой строки.
    #[test]
    fn folded_counter_survives_shifts() {
        let mut doc = Document::from_str("l0\nl1\nl2\n");
        doc.line_meta_mut().set_folded(2, true);
        doc.apply(&Transaction::from_edits(vec![Edit::insert(0, "x\ny\n")]));
        let meta = doc.line_meta();
        assert_eq!(meta.folded_count(), 1);
        assert_eq!(meta.folded_lines(), vec![4]);

        // Удаляем свёрнутую строку 4.
        let start = doc.line_to_char(4);
        let end = doc.line_to_char(5);
        doc.apply(&Transaction::from_edits(vec![Edit::delete(start, end)]));
        assert_eq!(doc.line_meta().folded_count(), 0);
        assert_eq!(doc.line_meta().line_count(), doc.len_lines());
    }

    /// Наивная модель (Vec + splice) — эталон для fuzz-сравнения treap.
    struct NaiveMeta(Vec<(bool, bool)>);

    impl NaiveMeta {
        fn insert(&mut self, pos: usize, count: usize) {
            self.0.splice(pos..pos, std::iter::repeat((false, false)).take(count));
        }
        fn remove(&mut self, pos: usize, count: usize) {
            self.0.drain(pos..pos + count);
        }
    }

    fn next(rng: &mut u64) -> u64 {
        *rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let z = *rng;
        (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9)
    }

    /// Fuzz: случайные транзакции и переключения флагов; treap сверяется
    /// с наивной моделью и с инвариантом line_count == len_lines.
    #[test]
    fn random_transactions_match_naive_model() {
        let mut rng: u64 = 0x0DDB_1A5E_5BAD_5EED;
        let mut doc = Document::from_str("");
        let mut meta = LineMeta::with_lines(doc.len_lines());
        let mut naive = NaiveMeta(vec![(false, false)]);

        for _ in 0..300 {
            if next(&mut rng) % 4 < 2 {
                // Переключения флагов на случайных строках.
                for _ in 0..1 + next(&mut rng) % 3 {
                    if naive.0.is_empty() {
                        break;
                    }
                    let line = (next(&mut rng) as usize) % naive.0.len();
                    if next(&mut rng) % 2 == 0 {
                        meta.toggle_bookmark(line);
                        naive.0[line].0 = !naive.0[line].0;
                    } else {
                        meta.toggle_folded(line);
                        naive.0[line].1 = !naive.0[line].1;
                    }
                }
            } else {
                // Случайная транзакция из 1-3 непересекающихся правок
                // по возрастанию (позиции — до-транзакционные).
                let len = doc.len_chars();
                let n_edits = 1 + next(&mut rng) % 3;
                let mut edits: Vec<Edit> = Vec::new();
                let mut cursor = 0usize;
                for _ in 0..n_edits {
                    let start = cursor + (next(&mut rng) as usize % (len - cursor + 1));
                    let end = if start < len && next(&mut rng) % 2 == 0 {
                        start + 1 + (next(&mut rng) as usize % (len - start))
                    } else {
                        start
                    };
                    let mut text = String::new();
                    for _ in 0..next(&mut rng) % 6 {
                        if next(&mut rng) % 5 == 0 {
                            text.push('\n');
                        } else {
                            text.push((b'a' + (next(&mut rng) % 26) as u8) as char);
                        }
                    }
                    edits.push(Edit { start, end, text });
                    cursor = end;
                }
                let tx = Transaction::from_edits(edits);

                // Зеркалим в наивную модель по до-транзакционным координатам
                // (то же правило привязки флагов, что и в apply_transaction).
                let mut delta: i64 = 0;
                for e in tx.edits() {
                    let l = doc.char_to_line(e.start);
                    let at_boundary = e.start == doc.line_to_char(l);
                    let line = (l as i64 + delta).max(0) as usize;
                    let edge = if at_boundary { line } else { line + 1 };
                    let removed = if e.end > e.start {
                        doc.rope().slice(e.start..e.end).chars().filter(|&c| c == '\n').count()
                    } else {
                        0
                    };
                    let inserted = e.text.chars().filter(|&c| c == '\n').count();
                    naive.remove(edge, removed);
                    naive.insert(edge, inserted);
                    delta += inserted as i64 - removed as i64;
                }
                meta.apply_transaction(&tx, doc.rope());
                doc.apply(&tx);

                assert_eq!(meta.line_count(), doc.len_lines(), "line_count vs len_lines");
                assert_eq!(meta.line_count(), naive.0.len(), "line_count vs naive");
                assert_eq!(
                    meta.bookmarked_lines(),
                    (0..naive.0.len()).filter(|&i| naive.0[i].0).collect::<Vec<_>>(),
                    "bookmarks"
                );
                assert_eq!(
                    meta.folded_lines(),
                    (0..naive.0.len()).filter(|&i| naive.0[i].1).collect::<Vec<_>>(),
                    "folds"
                );
                assert_eq!(meta.bookmark_count(), meta.bookmarked_lines().len());
                assert_eq!(meta.folded_count(), meta.folded_lines().len());
            }

            // Точечная сверка всех строк.
            for i in 0..naive.0.len() {
                assert_eq!(
                    (meta.is_bookmarked(i), meta.is_folded(i)),
                    naive.0[i],
                    "расхождение на строке {i}"
                );
            }
        }
    }

    /// Индекс, встроенный в Document, остаётся синхронным с rope.
    #[test]
    fn document_meta_stays_in_sync() {
        let mut doc = Document::from_str("a\nb\nc\nd\n");
        doc.line_meta_mut().set_bookmark(1);
        let tx = Transaction::from_edits(vec![Edit::insert(0, "X\n")]);
        let inverse = doc.apply(&tx);
        assert_eq!(doc.text(), "X\na\nb\nc\nd\n");
        assert_eq!(doc.len_lines(), 6);
        assert_eq!(doc.line_meta().line_count(), 6);
        assert_eq!(doc.line_meta().bookmarked_lines(), vec![2]);

        // Инверсия возвращает и текст, и позиции метаданных.
        doc.apply(&inverse);
        assert_eq!(doc.text(), "a\nb\nc\nd\n");
        assert_eq!(doc.line_meta().line_count(), 5);
        assert_eq!(doc.line_meta().bookmarked_lines(), vec![1]);
    }
}
