//! Модель вкладок (задача 3.2) — GPUI-free: каждая вкладка держит
//! документ, историю, курсоры (мульти-курсор, главный — последний) и
//! dirty-флаг; редактирование идёт
//! ТОЛЬКО через editor-core (Document + History + транзакции
//! selection.rs), каретка смещается правками через
//! `Transaction::map_selection`. Интеграционные тесты — без GUI.

use std::io;
use std::path::{Path, PathBuf};

use editor_core::{Document, History, Selection};
use editor_core::{backspace, forward_delete, insert_text};

/// Одна вкладка: документ + история + курсоры + dirty-флаг
/// (dirty ведётся здесь, на стороне tab-модели; editor-core не
/// расширяется — design.md D8.2).
#[derive(Debug)]
pub struct BufferTab {
    /// `None` — untitled-вкладка (первый Cmd+S → диалог сохранения).
    pub path: Option<PathBuf>,
    pub doc: Document,
    pub history: History,
    /// Курсоры (4.3): каретки и выделения. ГЛАВНЫЙ — ПОСЛЕДНИЙ элемент
    /// (добавление Cmd+клик/Cmd+D аппендит; D9.3).
    pub cursors: Vec<Selection>,
    /// Цель по столбцу для вертикальной навигации (4.2, D9.2):
    /// запоминается при первом движении вверх/вниз, сбрасывается любым
    /// горизонтальным движением/кликом/вводом. Только для одного курсора.
    pub goal_col: Option<usize>,
    pub dirty: bool,
    /// Scroll-смещение редактора (y, px) — для переключений вкладок
    /// и восстановления сессии (по желанию, D8.4).
    pub scroll_y: f32,
    /// Версия вида: растёт при любом изменении, влияющем на рендер
    /// строк (правки, undo/redo, фолды). Ключ кэша мапы visible→real
    /// в виджете (4.5/D9.5). Закладки мапу не меняют и версию не трогают.
    view_version: u64,
}

impl BufferTab {
    /// Версия вида (ключ кэша мапы folding, D9.5).
    pub fn view_version(&self) -> u64 {
        self.view_version
    }

    fn bump_view(&mut self) {
        self.view_version += 1;
    }

    /// Главная каретка (последний курсор).
    pub fn primary(&self) -> &Selection {
        self.cursors.last().expect("at least one cursor")
    }

    fn primary_mut(&mut self) -> &mut Selection {
        self.cursors.last_mut().expect("at least one cursor")
    }

    // ---- Конструирование, заголовок, сохранение ----

    /// Untitled-вкладка: пустой документ UTF-8/LF (дефолт FileFormat).
    pub fn untitled() -> Self {
        BufferTab {
            path: None,
            doc: Document::new(),
            history: History::new(),
            cursors: vec![Selection::caret(0)],
            goal_col: None,
            dirty: false,
            scroll_y: 0.0,
            view_version: 0,
        }
    }

    /// Открыть файл (кодировка/переводы строк — детект editor-core 2.5).
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let doc = Document::load_file_detected(&path)?;
        Ok(BufferTab {
            path: Some(path),
            doc,
            history: History::new(),
            cursors: vec![Selection::caret(0)],
            goal_col: None,
            dirty: false,
            scroll_y: 0.0,
            view_version: 0,
        })
    }

    /// Заголовок вкладки: имя файла или «Untitled N» (номер задаёт модель).
    pub fn title(&self, untitled_no: u32) -> String {
        match &self.path {
            Some(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.to_string_lossy().into_owned()),
            None => format!("Untitled {untitled_no}"),
        }
    }

    /// Сохранить в путь; успешное сохранение сбрасывает dirty и
    /// запоминает путь (для save-as и первого сохранения untitled).
    pub fn save_to(&mut self, path: impl Into<PathBuf>) -> io::Result<()> {
        let path = path.into();
        self.doc.save_file(&path)?;
        self.path = Some(path);
        self.dirty = false;
        Ok(())
    }

    /// Сохранить по собственному пути (untitled — ошибка, GUI сначала
    /// показывает save-as диалог).
    pub fn save(&mut self) -> io::Result<()> {
        let path = self.path.clone().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "untitled tab has no path")
        })?;
        self.doc.save_file(&path)?;
        self.dirty = false;
        Ok(())
    }

    // ---- Редактирование (через editor-core, все курсоры сразу) ----

    /// Ввести текст во все курсоры. Любая применённая правка делает
    /// вкладку dirty и регистрируется в History.
    pub fn insert_text(&mut self, text: &str) {
        let tx = insert_text(&self.cursors, text);
        self.apply_tx(tx);
    }

    /// Backspace: выделение удаляется целиком, иначе символ перед
    /// кареткой (каретка в 0 — no-op для этого курсора).
    pub fn backspace(&mut self) {
        let tx = backspace(self.doc.len_chars(), &self.cursors);
        self.apply_tx(tx);
    }

    /// Forward delete (каретка в конце — no-op).
    pub fn delete_forward(&mut self) {
        let tx = forward_delete(self.doc.len_chars(), &self.cursors);
        self.apply_tx(tx);
    }

    fn apply_tx(&mut self, tx: editor_core::Transaction) {
        if tx.is_empty() {
            return;
        }
        // Курсоры смещаются правкой в пост-координаты ДО применения —
        // позиции tx до-транзакционные. Единая политика виджета (D2.2
        // + D9.3): после ЛЮБОГО редактирования курсоры схлопываются в
        // каретки в головах; дубликаты (совпавшие после отброшенных
        // пересечений) удаляются — остаётся позднейший (он же главный).
        let mapped: Vec<Selection> =
            self.cursors.iter().map(|s| tx.map_selection(s)).collect();
        self.cursors = dedup_carets(mapped);
        self.goal_col = None;
        self.history.apply(&mut self.doc, &tx);
        self.dirty = true;
        self.bump_view();
    }

    /// Undo; курсоры клампятся в границы документа (точный маппинг
    /// сквозь undo-группу не нужен для MVP — D9.3).
    pub fn undo(&mut self) -> bool {
        let done = self.history.undo(&mut self.doc);
        if done {
            self.clamp_cursors();
            self.bump_view();
        }
        done
    }

    /// Redo; симметричен undo. Dirty не сбрасывается (нет baseline
    /// «сохранённого состояния» — D8.2).
    pub fn redo(&mut self) -> bool {
        let done = self.history.redo(&mut self.doc);
        if done {
            self.clamp_cursors();
            self.bump_view();
        }
        done
    }

    fn clamp_cursors(&mut self) {
        let len = self.doc.len_chars();
        for c in &mut self.cursors {
            let head = c.head().min(len);
            *c = Selection::caret(head);
        }
    }

    // ---- Навигация (4.2/4.3, GPUI-free) ----
    //
    // `extend: true` — расширение выделения ТОЛЬКО главной каретки
    // (якорь неподвижен, голова движется — D9.3); false — движение
    // ВСЕХ курсоров, выделения сбрасываются в каретки.

    pub fn move_left(&mut self, extend: bool) {
        self.goal_col = None;
        if extend {
            let head = self.primary().head();
            self.extend_primary(head.saturating_sub(1));
        } else {
            for c in &mut self.cursors {
                *c = Selection::caret(c.head().saturating_sub(1));
            }
        }
    }

    pub fn move_right(&mut self, extend: bool) {
        self.goal_col = None;
        if extend {
            let head = self.primary().head();
            self.extend_primary((head + 1).min(self.doc.len_chars()));
        } else {
            let len = self.doc.len_chars();
            for c in &mut self.cursors {
                *c = Selection::caret((c.head() + 1).min(len));
            }
        }
    }

    pub fn move_up(&mut self, extend: bool) {
        self.move_vertically(-1, extend);
    }

    pub fn move_down(&mut self, extend: bool) {
        self.move_vertically(1, extend);
    }

    /// PgUp/PgDn: `lines` — высота страницы в строках (считает виджет,
    /// D9.2); движение с сохранением goal-столбца, clamp по границам.
    pub fn move_page(&mut self, pages: isize, lines: usize, extend: bool) {
        let delta = pages * lines.max(1) as isize;
        if delta != 0 {
            self.move_vertically(delta, extend);
        }
    }

    pub fn move_home(&mut self, extend: bool) {
        self.goal_col = None;
        if extend {
            let head = self.primary().head();
            let line = self.doc.char_to_line(head);
            self.extend_primary(self.doc.line_to_char(line));
        } else {
            let starts: Vec<usize> = self
                .cursors
                .iter()
                .map(|c| {
                    let line = self.doc.char_to_line(c.head());
                    self.doc.line_to_char(line)
                })
                .collect();
            for (c, start) in self.cursors.iter_mut().zip(starts) {
                *c = Selection::caret(start);
            }
        }
    }

    pub fn move_end(&mut self, extend: bool) {
        self.goal_col = None;
        if extend {
            let head = self.primary().head();
            let line = self.doc.char_to_line(head);
            let end = self.doc.line_to_char(line) + self.line_len(line);
            self.extend_primary(end);
        } else {
            let ends: Vec<usize> = self
                .cursors
                .iter()
                .map(|c| {
                    let line = self.doc.char_to_line(c.head());
                    self.doc.line_to_char(line) + self.line_len(line)
                })
                .collect();
            for (c, end) in self.cursors.iter_mut().zip(ends) {
                *c = Selection::caret(end);
            }
        }
    }

    /// Клик мышью: ЕДИНСТВЕННАЯ каретка в (`line`, `col`) — все
    /// дополнительные курсоры сбрасываются (clamp по длине строки).
    pub fn set_caret_at(&mut self, line: usize, col: usize) {
        self.goal_col = None;
        let pos = self.clamp_pos(line, col);
        self.cursors = vec![Selection::caret(pos)];
    }

    /// Расширение выделения до позиции клика/драга: якорь главной
    /// каретки неподвижен, остальные курсоры сохраняются.
    pub fn select_to(&mut self, line: usize, col: usize) {
        self.goal_col = None;
        let pos = self.clamp_pos(line, col);
        self.extend_primary(pos);
    }

    // ---- Закладки (4.4; хранятся в LineMeta, переживают undo/redo) ----

    /// Cmd+F2: поставить/снять закладку на строке главной каретки.
    pub fn toggle_bookmark_on_primary(&mut self) {
        let line = self.cursor_line();
        self.doc.line_meta_mut().toggle_bookmark(line);
    }

    /// F2: следующая закладка по кругу от строки главной каретки.
    /// Каретка (единственная) переходит в начало строки закладки;
    /// `true` — переход случился.
    pub fn goto_next_bookmark(&mut self) -> bool {
        let from = self.cursor_line();
        self.doc
            .line_meta()
            .next_bookmark(from)
            .map(|line| {
                self.set_caret_at(line, 0);
                true
            })
            .unwrap_or(false)
    }

    /// Shift+F2: предыдущая закладка по кругу.
    pub fn goto_prev_bookmark(&mut self) -> bool {
        let from = self.cursor_line();
        self.doc
            .line_meta()
            .prev_bookmark(from)
            .map(|line| {
                self.set_caret_at(line, 0);
                true
            })
            .unwrap_or(false)
    }

    /// Строки с закладками активного файла (для панели, 4.4).
    pub fn bookmarked_lines(&self) -> Vec<usize> {
        self.doc.line_meta().bookmarked_lines()
    }

    /// Переключить закладку на строке (клик по маркеру в гуттере).
    pub fn toggle_bookmark_at(&mut self, line: usize) {
        self.doc.line_meta_mut().toggle_bookmark(line);
    }

    // ---- Операции со строками (4.6, D9.6) ----

    /// Строки, пересекаемые выделениями курсоров (не пустыми);
    /// `None` — выделений нет, операция над всем файлом.
    /// Выделение, заканчивающееся в колонке 0, строку не захватывает.
    fn selection_lines(&self) -> Option<(usize, usize)> {
        let mut scope: Option<(usize, usize)> = None;
        for sel in &self.cursors {
            if sel.is_caret() {
                continue;
            }
            let first = self.doc.char_to_line(sel.start);
            let mut last = self.doc.char_to_line(sel.end);
            if last > first && self.doc.line_to_char(last) == sel.end {
                last -= 1; // конец в колонке 0 строки `last`
            }
            scope = Some(match scope {
                None => (first, last),
                Some((f, l)) => (f.min(first), l.max(last)),
            });
        }
        scope
    }

    fn apply_block_op(
        &mut self,
        op: impl Fn(&editor_core::Document, usize, usize) -> editor_core::Transaction,
    ) {
        let (first, mut last) = self.selection_lines().unwrap_or_else(|| {
            (0, self.doc.len_lines().saturating_sub(1))
        });
        // Whole-file: фантомная хвостовая пустая строка ropey (после
        // завершающего \n) не участвует в операциях.
        if last > first && self.doc.line_str(last).is_empty() {
            last -= 1;
        }
        let tx = op(&self.doc, first, last);
        self.apply_tx(tx);
    }

    /// Сортировка строк выделения (или всего файла).
    pub fn sort_lines(&mut self, ascending: bool, ignore_case: bool) {
        self.apply_block_op(|doc, f, l| {
            editor_core::sort_lines(doc, f, l, ascending, ignore_case)
        });
    }

    /// Удаление дубликатов (порядок первых вхождений).
    pub fn dedupe_lines(&mut self) {
        self.apply_block_op(|doc, f, l| editor_core::dedupe_lines(doc, f, l));
    }

    /// Удаление пустых строк.
    pub fn remove_empty_lines(&mut self) {
        self.apply_block_op(|doc, f, l| editor_core::remove_empty_lines(doc, f, l));
    }

    /// Регистр: на выделениях (каждое — отдельный диапазон) или всём
    /// файле, если непустых выделений нет.
    pub fn change_case(&mut self, mode: editor_core::CaseMode) {
        let ranges: Vec<(usize, usize)> = self
            .cursors
            .iter()
            .filter(|s| !s.is_caret())
            .map(|s| (s.start, s.end))
            .collect();
        let tx = if ranges.is_empty() {
            editor_core::change_case(&self.doc, &[(0, self.doc.len_chars())], mode)
        } else {
            editor_core::change_case(&self.doc, &ranges, mode)
        };
        self.apply_tx(tx);
    }

    // ---- Свёртывание (4.5, D9.5) ----

    /// Toggle фолда на строке-заголовке (клик по маркеру). Не-заголовок
    /// — no-op.
    pub fn toggle_fold(&mut self, line: usize) {
        if crate::folding::is_foldable(&self.doc, line) {
            self.doc.line_meta_mut().toggle_folded(line);
            self.bump_view();
        }
    }

    /// Свернуть все блоки (заголовки по отступам).
    pub fn fold_all(&mut self) {
        let headers: Vec<usize> = (0..self.doc.len_lines())
            .filter(|l| crate::folding::is_foldable(&self.doc, *l))
            .collect();
        {
            let meta = self.doc.line_meta_mut();
            for l in headers {
                meta.set_folded(l, true);
            }
        }
        self.bump_view();
    }

    /// Развернуть всё.
    pub fn unfold_all(&mut self) {
        self.doc.line_meta_mut().folded_lines().iter().for_each(|l| {
            self.doc.line_meta_mut().set_folded(*l, false);
        });
        self.bump_view();
    }

    /// Строка видима? (не внутри свёрнутого тела)
    #[allow(dead_code)] // часть API модели (проверки видимости из виджета)
    pub fn is_line_visible(&self, line: usize) -> bool {
        crate::folding::enclosing_fold_header(&self.doc, line).is_none()
    }

    /// Ближайшая ВИДИМАЯ строка: скрытая строка → заголовок её фолда.
    /// Для движения ВВЕРХ и перехода к строке (Cmd+G).
    pub fn visible_line_or_header(&self, line: usize) -> usize {
        crate::folding::enclosing_fold_header(&self.doc, line).unwrap_or(line)
    }

    /// Первая видимая строка НАЧИНАЯ С `line` (для движения ВНИЗ:
    /// тело свёрнутого блока под текущей строкой перепрыгивается).
    pub fn first_visible_at_or_after(&self, line: usize) -> usize {
        let mut l = line;
        for _ in 0..64 {
            let Some(header) = crate::folding::enclosing_fold_header(&self.doc, l) else {
                return l;
            };
            l = crate::folding::fold_end(&self.doc, header) + 1;
        }
        self.doc.len_lines().saturating_sub(1)
    }

    // ---- Мульти-курсор (4.3, D9.3) ----

    /// Жесты мыши (D9.3): двойной клик — слово, тройной — строка.

    /// Двойной клик: выделить слово под позицией (нет слова — каретка).
    pub fn select_word_at(&mut self, line: usize, col: usize) {
        self.goal_col = None;
        let pos = self.clamp_pos(line, col);
        self.cursors = match self.word_at(pos) {
            Some((s, e)) => vec![Selection::range(s, e)],
            None => vec![Selection::caret(pos)],
        };
    }

    /// Тройной клик: выделить строку целиком (с терминатором).
    pub fn select_line_at(&mut self, line: usize) {
        self.goal_col = None;
        let line = line.min(self.doc.len_lines().saturating_sub(1));
        let start = self.doc.line_to_char(line);
        let end = if line + 1 < self.doc.len_lines() {
            self.doc.line_to_char(line + 1)
        } else {
            self.doc.len_chars()
        };
        self.cursors = vec![Selection::range(start, end)];
    }

    /// Заменить весь набор курсоров (якорь/головы уже в char-координатах).
    #[allow(dead_code)] // установка набора кареток тестами/GUI напрямую
    pub fn set_cursors(&mut self, cursors: Vec<Selection>) {
        self.goal_col = None;
        if cursors.is_empty() {
            return;
        }
        self.cursors = cursors;
    }

    /// Cmd+клик: добавить каретку в позицию. Повторный Cmd+клик в ту же
    /// точку снимает курсор (toggle, как в VS Code); совпадение с
    /// существующей кареткой без Cmd — обычный клик, сюда не попадает.
    pub fn add_cursor_at(&mut self, line: usize, col: usize) {
        self.goal_col = None;
        let pos = self.clamp_pos(line, col);
        let caret = Selection::caret(pos);
        if let Some(i) = self.cursors.iter().position(|c| *c == caret) {
            if self.cursors.len() > 1 {
                self.cursors.remove(i);
            }
            return;
        }
        self.cursors.push(caret);
    }

    /// Column selection (Alt/Cmd+drag): прямоугольник между якорем и
    /// головой → каретки/выделения в начале каждой строки блока.
    /// Короткие строки дают каретку в их конце (VS Code-семантика).
    pub fn set_column_block(&mut self, anchor: (usize, usize), head: (usize, usize)) {
        self.goal_col = None;
        self.cursors = column_block(&self.doc, anchor, head);
    }

    /// Cmd+D: первый вызов — выделить слово под главной кареткой;
    /// следующий — добавить следующее вхождение того же текста
    /// (циклически, без пересечений с существующими курсорами).
    /// `false` — добавлять нечего.
    pub fn add_next_word_match(&mut self) -> bool {
        let primary = *self.primary();
        if primary.is_caret() {
            if let Some((s, e)) = self.word_at(primary.head()) {
                *self.primary_mut() = Selection::range(s, e);
                return true;
            }
            return false;
        }
        let needle = self.doc.slice_str(primary.start, primary.end);
        if needle.is_empty() {
            return false;
        }
        let len = self.doc.len_chars();
        // После конца главного выделения, затем циклически — с начала
        // и до его начала.
        let found = self
            .find_from(&needle, primary.end, len)
            .or_else(|| self.find_from(&needle, 0, primary.start));
        if let Some((s, e)) = found {
            let overlaps = self
                .cursors
                .iter()
                .any(|sel| s < sel.end && e > sel.start);
            if !overlaps {
                self.cursors.push(Selection::range(s, e));
                return true;
            }
        }
        false
    }

    // ---- Внутренние хелперы ----

    /// Слово под позицией (границы: буква/цифра/подчёркивание).
    /// Каретка сразу после слова — берём слово слева.
    fn word_at(&self, pos: usize) -> Option<(usize, usize)> {
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let char_at = |p: usize| -> Option<char> {
            if p < self.doc.len_chars() {
                self.doc.slice_str(p, p + 1).chars().next()
            } else {
                None
            }
        };
        if char_at(pos).is_some_and(is_word) {
            let mut s = pos;
            while s > 0 && char_at(s - 1).is_some_and(is_word) {
                s -= 1;
            }
            let mut e = pos;
            let len = self.doc.len_chars();
            while e < len && char_at(e).is_some_and(is_word) {
                e += 1;
            }
            return Some((s, e));
        }
        if pos > 0 && char_at(pos - 1).is_some_and(is_word) {
            let mut s = pos;
            while s > 0 && char_at(s - 1).is_some_and(is_word) {
                s -= 1;
            }
            return Some((s, pos));
        }
        None
    }

    /// Первое вхождение `needle` в char-диапазоне `[from, to)`;
    /// `to < from` → None.
    fn find_from(&self, needle: &str, from: usize, to: usize) -> Option<(usize, usize)> {
        if to <= from {
            return None;
        }
        let hay = self.doc.slice_str(from, to);
        let byte_idx = hay.find(needle)?;
        let start = from + hay[..byte_idx].chars().count();
        Some((start, start + needle.chars().count()))
    }

    /// Поставить голову главной каретки, сохранив якорь.
    fn extend_primary(&mut self, pos: usize) {
        let anchor = self.primary().anchor();
        *self.primary_mut() = Selection::range(anchor, pos);
    }

    fn clamp_pos(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.doc.len_lines().saturating_sub(1));
        let col = col.min(self.line_len(line));
        self.doc.line_to_char(line) + col
    }

    fn move_vertically(&mut self, delta: isize, extend: bool) {
        let lines = self.doc.len_lines();
        // Вертикальное движение не заходит в свёрнутые тела (4.5):
        // вниз — первая видимая от цели, вверх — заголовок блока.
        let target_line = |tab: &BufferTab, line: usize| {
            let clamped = if delta < 0 {
                line.saturating_sub(delta.unsigned_abs())
            } else {
                (line + delta.unsigned_abs()).min(lines.saturating_sub(1))
            };
            if delta < 0 {
                tab.visible_line_or_header(clamped)
            } else {
                tab.first_visible_at_or_after(clamped)
            }
        };
        if extend {
            // Расширяется только главная каретка (D9.3), с goal-столбцом.
            let head = self.primary().head();
            let line = self.doc.char_to_line(head);
            let col = head - self.doc.line_to_char(line);
            let goal = *self.goal_col.get_or_insert(col);
            let new_line = target_line(self, line);
            self.extend_primary(self.doc.line_to_char(new_line) + goal.min(self.line_len(new_line)));
            return;
        }
        if self.cursors.len() == 1 {
            // Одиночная каретка: goal column работает (D9.2).
            let head = self.cursors[0].head();
            let line = self.doc.char_to_line(head);
            let col = head - self.doc.line_to_char(line);
            let goal = *self.goal_col.get_or_insert(col);
            let new_line = target_line(self, line);
            let new_col = goal.min(self.line_len(new_line));
            self.cursors[0] = Selection::caret(self.doc.line_to_char(new_line) + new_col);
        } else {
            // Мульти-курсор: goal-столбец у каждого свой — не храним,
            // клампим по столбцу текущей строки (D9.3).
            let moved: Vec<Selection> = self
                .cursors
                .iter()
                .map(|c| {
                    let head = c.head();
                    let line = self.doc.char_to_line(head);
                    let col = head - self.doc.line_to_char(line);
                    let new_line = target_line(self, line);
                    Selection::caret(
                        self.doc.line_to_char(new_line) + col.min(self.line_len(new_line)),
                    )
                })
                .collect();
            self.cursors = moved;
        }
    }

    /// Строка ГЛАВНОЙ каретки (0-based).
    pub fn cursor_line(&self) -> usize {
        self.doc.char_to_line(self.primary().head())
    }

    /// Столбец ГЛАВНОЙ каретки (0-based, char-единицы).
    pub fn cursor_col(&self) -> usize {
        let line = self.cursor_line();
        self.primary().head() - self.doc.line_to_char(line)
    }

    /// Длина строки без терминатора.
    fn line_len(&self, line: usize) -> usize {
        self.doc.line_content(line).chars().count()
    }
}

/// После редактирования: все курсоры → каретки в головах; дубликаты
/// удаляются с сохранением ПОЗДНЕГО вхождения (он же главный, D9.3).
fn dedup_carets(sels: Vec<Selection>) -> Vec<Selection> {
    let carets: Vec<Selection> = sels.iter().map(|s| Selection::caret(s.head())).collect();
    let mut out: Vec<Selection> = Vec::with_capacity(carets.len());
    for (i, c) in carets.iter().enumerate() {
        if carets[i + 1..].contains(c) {
            continue; // такой же курсор позже — этот дропаем
        }
        if out.last() != Some(c) {
            out.push(*c);
        }
    }
    if out.is_empty() {
        out.push(Selection::caret(0));
    }
    out
}

/// Прямоугольник (якорь, голова) → курсоры на каждую строку блока:
/// выделение [min_col..max_col), кламп по длине строки; строка короче
/// min_col → каретка в её конце.
fn column_block(doc: &Document, anchor: (usize, usize), head: (usize, usize)) -> Vec<Selection> {
    let (l0, l1) = if anchor.0 <= head.0 { (anchor.0, head.0) } else { (head.0, anchor.0) };
    let (c0, c1) = if anchor.1 <= head.1 { (anchor.1, head.1) } else { (head.1, anchor.1) };
    let last_line = doc.len_lines().saturating_sub(1);
    let mut out = Vec::with_capacity(l1 - l0 + 1);
    for line in l0..=l1.min(last_line) {
        let len = doc.line_content(line).chars().count();
        let start = doc.line_to_char(line);
        let s = c0.min(len);
        let e = c1.min(len);
        out.push(if s == e {
            Selection::caret(start + s)
        } else {
            Selection::range(start + s, start + e)
        });
    }
    if out.is_empty() {
        out.push(Selection::caret(0));
    }
    out
}

/// Модель вкладок: открытие/закрытие/переключение/перестановка,
/// активный индекс, dirty-флаги, untitled-нумерация.
#[derive(Debug, Default)]
pub struct TabModel {
    tabs: Vec<BufferTab>,
    active: Option<usize>,
    next_untitled: u32,
}

impl TabModel {
    pub fn new() -> Self {
        TabModel::default()
    }

    pub fn tabs(&self) -> &[BufferTab] {
        &self.tabs
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn active_index(&self) -> Option<usize> {
        self.active
    }

    pub fn active(&self) -> Option<&BufferTab> {
        self.active.and_then(|i| self.tabs.get(i))
    }

    pub fn active_mut(&mut self) -> Option<&mut BufferTab> {
        let i = self.active?;
        self.tabs.get_mut(i)
    }

    /// Мутабельный доступ к вкладке по индексу (для save-цепочек).
    pub fn tab_mut(&mut self, index: usize) -> Option<&mut BufferTab> {
        self.tabs.get_mut(index)
    }

    /// Заголовок вкладки по индексу (untitled-номер вычисляется по
    /// порядку среди untitled-вкладок).
    pub fn title(&self, index: usize) -> Option<String> {
        let tab = self.tabs.get(index)?;
        let no = self.untitled_no(index);
        Some(tab.title(no))
    }

    fn untitled_no(&self, index: usize) -> u32 {
        let mut no = 0;
        for (i, t) in self.tabs.iter().enumerate() {
            if t.path.is_none() {
                no += 1;
                if i >= index {
                    break;
                }
            }
        }
        no.max(1)
    }

    /// Открыть файл: если путь уже открыт — активировать существующую
    /// вкладку (дедупликация без каноникализации — диалоги ОС дают
    /// абсолютные нормализованные пути; D8.2), иначе новую вкладку
    /// и активировать её. Ошибка загрузки — наружу (GUI покажет).
    pub fn open_path(&mut self, path: impl AsRef<Path>) -> io::Result<usize> {
        let path = path.as_ref();
        if let Some(i) = self.tabs.iter().position(|t| t.path.as_deref() == Some(path)) {
            self.active = Some(i);
            return Ok(i);
        }
        let tab = BufferTab::open(path)?;
        self.tabs.push(tab);
        let i = self.tabs.len() - 1;
        self.active = Some(i);
        Ok(i)
    }

    /// Новая untitled-вкладка (активируется).
    pub fn new_untitled(&mut self) -> usize {
        self.next_untitled += 1;
        self.tabs.push(BufferTab::untitled());
        let i = self.tabs.len() - 1;
        self.active = Some(i);
        i
    }

    /// Активировать вкладку (границы — no-op).
    pub fn activate(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = Some(index);
        }
    }

    /// Закрыть вкладку по индексу. Активной становится правая соседка,
    /// если закрыли последнюю — левая; закрытие активной последней
    /// вкладки оставляет active = None.
    pub fn close(&mut self, index: usize) -> Option<BufferTab> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.active = None;
        } else if let Some(a) = self.active {
            self.active = Some(if index < a {
                a - 1
            } else if index == a {
                a.min(self.tabs.len() - 1)
            } else {
                a
            });
        }
        Some(tab)
    }

    /// Перестановка: сдвинуть активную вкладку на `offset` позиций
    /// (кнопки ‹ › в таб-баре, D8.2; drag-and-drop — позже). Возвращает
    /// новый индекс активной вкладки.
    pub fn move_active(&mut self, offset: isize) -> Option<usize> {
        let from = self.active?;
        let to = if offset < 0 {
            from.saturating_sub(offset.unsigned_abs())
        } else {
            (from + offset.unsigned_abs()).min(self.tabs.len() - 1)
        };
        if from == to {
            return Some(from);
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = Some(to);
        Some(to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// Открытие, переключение, закрытие; активный индекс после закрытия.
    #[test]
    fn open_switch_close() {
        let dir = TempDir::new("open-switch-close");
        let a = dir.write("a.txt", "alpha\n");
        let b = dir.write("b.txt", "beta\n");
        let c = dir.write("c.txt", "gamma\n");

        let mut m = TabModel::new();
        assert!(m.is_empty());
        assert_eq!(m.active_index(), None);

        m.open_path(&a).unwrap();
        m.open_path(&b).unwrap();
        m.open_path(&c).unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m.active_index(), Some(2));
        assert_eq!(m.title(2).unwrap(), "c.txt");

        m.activate(0);
        assert_eq!(m.active_index(), Some(0));
        m.activate(99); // вне диапазона — no-op
        assert_eq!(m.active_index(), Some(0));

        // Закрытие средней (не активной) — активная смещается влево.
        m.close(1);
        assert_eq!(m.len(), 2);
        assert_eq!(m.active_index(), Some(0));
        assert_eq!(m.title(1).unwrap(), "c.txt");

        // Закрытие активной (0) — активной становится правая соседка.
        m.close(0);
        assert_eq!(m.len(), 1);
        assert_eq!(m.active_index(), Some(0));
        assert_eq!(m.title(0).unwrap(), "c.txt");

        // Последняя вкладка: active -> None.
        m.close(0);
        assert!(m.is_empty());
        assert_eq!(m.active_index(), None);
        assert!(m.active().is_none());
    }

    /// Повторное открытие того же пути активирует существующую вкладку.
    #[test]
    fn reopen_same_path_activates_existing_tab() {
        let dir = TempDir::new("dedup");
        let a = dir.write("a.txt", "content\n");
        let mut m = TabModel::new();
        m.open_path(&a).unwrap();
        m.new_untitled();
        let i = m.open_path(&a).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m.active_index(), Some(0));
        assert_eq!(i, 0);
    }

    /// Untitled-вкладки: нумерация заголовков по порядку.
    #[test]
    fn untitled_tabs_numbering() {
        let dir = TempDir::new("untitled");
        let a = dir.write("a.txt", "x");
        let mut m = TabModel::new();
        m.new_untitled();
        m.new_untitled();
        m.open_path(&a).unwrap();
        m.new_untitled();
        assert_eq!(m.title(0).unwrap(), "Untitled 1");
        assert_eq!(m.title(1).unwrap(), "Untitled 2");
        assert_eq!(m.title(2).unwrap(), "a.txt");
        assert_eq!(m.title(3).unwrap(), "Untitled 3");

        // Закрытие untitled 1 перенумерует оставшиеся.
        m.close(0);
        assert_eq!(m.title(0).unwrap(), "Untitled 1"); // бывшая Untitled 2
        assert_eq!(m.title(2).unwrap(), "Untitled 2"); // бывшая 3
    }

    /// Untitled-вкладка: пустой документ, UTF-8/LF (дефолт FileFormat).
    #[test]
    fn untitled_tab_is_utf8_lf() {
        let t = BufferTab::untitled();
        assert_eq!(t.doc.text(), "");
        assert_eq!(t.doc.encoding().label(), "UTF-8");
        assert_eq!(t.doc.line_endings().dominant().label(), "LF");
        assert!(!t.dirty);
        assert_eq!(*t.primary(), Selection::caret(0));
    }

    /// Перестановка вкладок кнопками (move_active), границы — no-op.
    #[test]
    fn reorder_tabs() {
        let dir = TempDir::new("reorder");
        let a = dir.write("a.txt", "a");
        let b = dir.write("b.txt", "b");
        let c = dir.write("c.txt", "c");
        let mut m = TabModel::new();
        for p in [&a, &b, &c] {
            m.open_path(p).unwrap();
        }
        assert_eq!(m.active_index(), Some(2));

        assert_eq!(m.move_active(-1), Some(1)); // c <-> b
        assert_eq!(
            m.tabs().iter().map(|t| m_title(t)).collect::<Vec<_>>(),
            vec!["a.txt", "c.txt", "b.txt"]
        );
        assert_eq!(m.active_index(), Some(1));
        assert_eq!(m.active().unwrap().path.as_deref(), Some(c.as_path()));

        m.move_active(5); // clamp в конец
        assert_eq!(m.active_index(), Some(2));
        m.move_active(-5); // clamp в начало — активная (c) уезжает в начало
        assert_eq!(m.active_index(), Some(0));
        assert_eq!(m_title(&m.tabs()[0]), "c.txt");
        assert_eq!(
            m.tabs().iter().map(|t| m_title(t)).collect::<Vec<_>>(),
            vec!["c.txt", "a.txt", "b.txt"]
        );

        // Перестановка неактивной вкладки не предусмотрена (кнопки
        // двигают активную) — активная следует за вкладкой.
    }

    fn m_title(t: &BufferTab) -> String {
        t.title(0)
    }

    /// Dirty: правка ставит флаг, успешное сохранение сбрасывает;
    /// save_as запоминает новый путь.
    #[test]
    fn dirty_flag_lifecycle() {
        let dir = TempDir::new("dirty");
        let a = dir.write("a.txt", "hello");
        let mut m = TabModel::new();
        m.open_path(&a).unwrap();
        assert!(!m.active().unwrap().dirty);

        // Ввод в конец файла (каретка после открытия в начале).
        let tab = m.active_mut().unwrap();
        tab.set_caret_at(0, tab.doc.line_content(0).chars().count());
        tab.insert_text("!");
        assert!(m.active().unwrap().dirty);
        assert_eq!(m.active().unwrap().doc.text(), "hello!");

        // Сохранение сбрасывает.
        m.active_mut().unwrap().save().unwrap();
        assert!(!m.active().unwrap().dirty);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "hello!");

        // Save-as на untitled: путь запоминается, dirty сбрасывается.
        m.new_untitled();
        m.active_mut().unwrap().insert_text("new file");
        let target = dir.path().join("saved.txt");
        m.active_mut().unwrap().save_to(&target).unwrap();
        assert!(!m.active().unwrap().dirty);
        assert_eq!(m.active().unwrap().path.as_deref(), Some(target.as_path()));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new file");
    }

    /// Сохранение untitled без пути — ошибка (GUI сначала save-as).
    #[test]
    fn save_untitled_without_path_is_error() {
        let mut t = BufferTab::untitled();
        let err = t.save().unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    /// Редактирование через editor-core: ввод/undo/redo, каретка
    /// смещается правками (map_selection), документ LF-нормализован.
    #[test]
    fn editing_through_history() {
        let mut t = BufferTab::untitled();
        for ch in "hello".chars() {
            t.insert_text(&ch.to_string());
        }
        assert_eq!(t.doc.text(), "hello");
        assert_eq!(*t.primary(), Selection::caret(5));
        assert!(t.dirty);

        // Каретка в середину, ввод — позиция каретки честно уезжает.
        // Одночные вставки продолжают ту же undo-группу (D2.4), поэтому
        // один undo снимает весь ввод целиком.
        t.set_caret_at(0, 2);
        t.insert_text("X");
        assert_eq!(t.doc.text(), "heXllo");
        assert_eq!(*t.primary(), Selection::caret(3));

        assert!(t.undo());
        assert_eq!(t.doc.text(), "");
        assert!(t.primary().head() <= 5);
        assert!(t.redo());
        assert_eq!(t.doc.text(), "heXllo");
        // Redo закрыл группу: результат можно снова undo (D2.4).
        assert!(t.undo());
        assert_eq!(t.doc.text(), "");

        // Составная вставка — своя группа: «hello», затем «XY» — два undo.
        let mut t = BufferTab::untitled();
        for ch in "hello".chars() {
            t.insert_text(&ch.to_string());
        }
        t.insert_text("XY"); // мульти-символьная — закрывает группу
        assert_eq!(t.doc.text(), "helloXY");
        assert!(t.undo());
        assert_eq!(t.doc.text(), "hello");
        assert!(t.undo());
        assert_eq!(t.doc.text(), "");
        // Dirty не сбрасывается undo (нет baseline, D8.2).
        assert!(t.dirty);
    }

    /// 4.3: ввод на нескольких каретках; после правки курсоры —
    /// каретки за вставленным текстом (D2.2/D9.3).
    #[test]
    fn multi_cursor_typing() {
        let mut t = BufferTab::from_text("one two three");
        t.set_cursors(vec![
            Selection::caret(3),
            Selection::caret(7),
            Selection::caret(13),
        ]);
        t.insert_text(",");
        assert_eq!(t.doc.text(), "one, two, three,");
        assert_eq!(
            t.cursors,
            vec![
                Selection::caret(4),
                Selection::caret(9),
                Selection::caret(16),
            ]
        );
        // Один undo снимает всю правку (одна группа: одиночные вставки).
        assert!(t.undo());
        assert_eq!(t.doc.text(), "one two three");
    }

    /// 4.3: ввод ПОВЕРХ выделений схлопывает их в каретки
    /// (политика виджета поверх map_selection — D2.2/D9.3).
    #[test]
    fn insert_over_selections_collapses_to_carets() {
        let mut t = BufferTab::from_text("alpha beta gamma");
        t.set_cursors(vec![
            Selection::range(0, 5),
            Selection::range(6, 10),
            Selection::range(11, 16),
        ]);
        t.insert_text("X");
        assert_eq!(t.doc.text(), "X X X");
        // Каретки за каждым вставленным X (Bias::Right): 1, 3, 5.
        assert_eq!(
            t.cursors,
            vec![Selection::caret(1), Selection::caret(3), Selection::caret(5)]
        );
    }

    /// 4.3: backspace на нескольких каретках; совпавшие каретки
    /// дедуплицируется (остаётся позднейший).
    #[test]
    fn multi_cursor_backspace_dedup() {
        let mut t = BufferTab::from_text("ab");
        t.set_cursors(vec![Selection::caret(1), Selection::caret(2)]);
        t.backspace(); // каждая каретка удаляет символ перед собой
        assert_eq!(t.doc.text(), "");
        // Обе каретки сошлись в 0 — остаётся одна.
        assert_eq!(t.cursors, vec![Selection::caret(0)]);
    }

    /// 4.3: Cmd+клик — toggle курсора в точке.
    #[test]
    fn add_cursor_at_toggles() {
        let mut t = BufferTab::from_text("one\ntwo\n");
        t.add_cursor_at(1, 0);
        assert_eq!(t.cursors.len(), 2);
        assert_eq!(*t.primary(), Selection::caret(4)); // главный — последний
        t.add_cursor_at(1, 0); // повторный — снимает
        assert_eq!(t.cursors.len(), 1);
        // Последний курсор снимать нельзя — остаётся один.
        t.add_cursor_at(0, 0);
        t.add_cursor_at(0, 0);
        assert_eq!(t.cursors.len(), 1);
    }

    /// 4.3: column selection — позиции по строкам блока, короткие
    /// строки дают каретку в конце. "abcdef\nab\nabcdefgh\n":
    /// line0 старт 0, line1 — 7, line2 — 10.
    #[test]
    fn column_block_positions() {
        let doc = Document::from_str("abcdef\nab\nabcdefgh\n");
        // Блок строк 0..=2, колонки 2..5.
        let sels = column_block(&doc, (0, 2), (2, 5));
        assert_eq!(
            sels,
            vec![
                Selection::range(2, 5),     // "abcdef": [2..5)
                Selection::caret(7 + 2),    // "ab" (len 2): каретка в конце
                Selection::range(10 + 2, 10 + 5), // "abcdefgh": [2..5)
            ]
        );
        // Обратное направление (голова выше якоря) — тот же блок.
        let sels_rev = column_block(&doc, (2, 5), (0, 2));
        assert_eq!(sels_rev, sels);
        // Блок из одной строки.
        let one = column_block(&doc, (1, 0), (1, 2));
        assert_eq!(one, vec![Selection::range(7, 9)]);
    }

    /// 4.3: Cmd+D — первый вызов выделяет слово под кареткой, затем
    /// добавляет следующие вхождения; исчерпание — false.
    #[test]
    fn cmd_d_word_matches() {
        let mut t = BufferTab::from_text("foo bar foo baz foo");
        assert!(t.add_next_word_match()); // "foo" [0..3)
        assert_eq!(*t.primary(), Selection::range(0, 3));
        assert!(t.add_next_word_match()); // [8..11)
        assert_eq!(t.cursors.len(), 2);
        assert_eq!(*t.primary(), Selection::range(8, 11));
        assert!(t.add_next_word_match()); // [16..19)
        assert_eq!(*t.primary(), Selection::range(16, 19));
        // Больше вхождений нет — false, курсоры не меняются.
        assert!(!t.add_next_word_match());
        assert_eq!(t.cursors.len(), 3);
        // Ввод на трёх выделениях заменяет их.
        t.insert_text("XX");
        assert_eq!(t.doc.text(), "XX bar XX baz XX");
    }

    /// 4.3: слово под кареткой в середине и сразу после слова.
    #[test]
    fn word_boundaries() {
        let mut t = BufferTab::from_text("fn main() { let x = foo_bar1; }");
        t.set_caret_at(0, 4); // внутри "main"
        assert!(t.add_next_word_match());
        assert_eq!(*t.primary(), Selection::range(3, 7));
        // Каретка сразу ПОСЛЕ слова (на '(').
        t.set_caret_at(0, 7);
        t.set_cursors(vec![Selection::caret(7)]);
        assert!(t.add_next_word_match());
        assert_eq!(*t.primary(), Selection::range(3, 7));
        // Не слово и не сразу после слова (пробел после '(') — false.
        t.set_cursors(vec![Selection::caret(8)]);
        assert!(!t.add_next_word_match());
    }

    /// 4.3: движение без shift двигает ВСЕ каретки; shift расширяет
    /// только главную.
    #[test]
    fn multi_cursor_movement() {
        let mut t = BufferTab::from_text("abcd\nxy\n");
        t.set_cursors(vec![Selection::caret(1), Selection::caret(7)]);
        t.move_left(false);
        assert_eq!(t.cursors, vec![Selection::caret(0), Selection::caret(6)]);
        t.move_right(false);
        t.move_down(false); // 1 → line1 col1 (6); 7 → хвост (2,0) = 8
        assert_eq!(t.cursors, vec![Selection::caret(6), Selection::caret(8)]);
    }

    /// 4.3: shift-движение с несколькими курсорами расширяет только
    /// главный, прочие неподвижны.
    #[test]
    fn shift_extends_primary_only() {
        let mut t = BufferTab::from_text("abcd\nxy\n");
        t.set_cursors(vec![Selection::caret(1), Selection::caret(6)]);
        t.move_right(true);
        assert_eq!(t.cursors[0], Selection::caret(1));
        assert_eq!(t.cursors[1], Selection::range(6, 7));
    }

    /// 4.4 (обязательный): закладки переживают undo/redo — флаги
    /// привязаны к содержимому строк и синхронизируются в Document::apply.
    #[test]
    fn bookmarks_survive_undo_redo() {
        let mut t = BufferTab::from_text("alpha\nbeta\ngamma\n");
        t.set_caret_at(1, 0);
        t.toggle_bookmark_on_primary();
        assert!(t.doc.line_meta().is_bookmarked(1));
        // Вставка двух строк ВЫШЕ закладки: флаг уезжает за содержимым.
        t.set_caret_at(0, 0);
        t.insert_text("x\ny\n");
        assert_eq!(t.doc.text(), "x\ny\nalpha\nbeta\ngamma\n");
        assert!(t.doc.line_meta().is_bookmarked(3), "bookmark follows line content");
        // Undo возвращает текст И позицию закладки.
        assert!(t.undo());
        assert_eq!(t.doc.text(), "alpha\nbeta\ngamma\n");
        assert!(t.doc.line_meta().is_bookmarked(1));
        assert!(!t.doc.line_meta().is_bookmarked(3));
        // Redo — снова за содержимым.
        assert!(t.redo());
        assert!(t.doc.line_meta().is_bookmarked(3));
        // Повторный undo — снова к строке 1.
        assert!(t.undo());
        assert!(t.doc.line_meta().is_bookmarked(1));
    }

    /// 4.4: F2/Shift+F2 — навигация по закладкам по кругу.
    #[test]
    fn bookmark_navigation_wraps() {
        let mut t = BufferTab::from_text("a\nb\nc\nd\ne\n");
        t.toggle_bookmark_at(1);
        t.toggle_bookmark_at(3);
        assert_eq!(t.bookmarked_lines(), vec![1, 3]);
        // Со строки 0: вперёд → 1, вперёд → 3, вперёд → 1 (по кругу).
        t.set_caret_at(0, 0);
        assert!(t.goto_next_bookmark());
        assert_eq!(t.cursor_line(), 1);
        assert!(t.goto_next_bookmark());
        assert_eq!(t.cursor_line(), 3);
        assert!(t.goto_next_bookmark());
        assert_eq!(t.cursor_line(), 1);
        // Назад: 3, затем 1, затем 3 (по кругу).
        assert!(t.goto_prev_bookmark());
        assert_eq!(t.cursor_line(), 3);
        assert!(t.goto_prev_bookmark());
        assert_eq!(t.cursor_line(), 1);
        assert!(t.goto_prev_bookmark());
        assert_eq!(t.cursor_line(), 3);
        // Навигация ставит каретку в начало строки.
        assert_eq!(t.cursor_col(), 0);
        // Без закладок — false.
        t.toggle_bookmark_at(1);
        t.toggle_bookmark_at(3);
        assert!(!t.goto_next_bookmark());
        assert!(!t.goto_prev_bookmark());
    }

    /// 4.4: toggle на строке главной каретки; клик по маркеру гуттера —
    /// toggle на произвольной строке.
    #[test]
    fn bookmark_toggle_paths() {
        let mut t = BufferTab::from_text("one\ntwo\nthree\n");
        t.set_caret_at(2, 1);
        t.toggle_bookmark_on_primary();
        assert_eq!(t.bookmarked_lines(), vec![2]);
        t.toggle_bookmark_on_primary(); // снять
        assert!(t.bookmarked_lines().is_empty());
        t.toggle_bookmark_at(0); // как клик по маркеру
        assert_eq!(t.bookmarked_lines(), vec![0]);
    }

    /// 4.5: toggle_fold скрывает тело; вертикальное движение не заходит
    /// в свёрнутые тела; fold_all/unfold_all.
    #[test]
    fn folding_model_navigation() {
        let mut t = BufferTab::from_text(
            "fn main() {\n    if a {\n        b();\n    }\n    tail();\n}\ntop();\n",
        );
        // Строки: 0 fn, 1 if, 2 b(), 3 }, 4 tail, 5 }, 6 top, 7 "".
        assert!(t.is_line_visible(2));
        t.toggle_fold(1); // тело if: строка 2
        assert!(t.doc.line_meta().is_folded(1));
        assert!(!t.is_line_visible(2));
        // Движение вниз со строки 1 → заголовок внешнего хвоста (4), не 2.
        t.set_caret_at(1, 0);
        t.move_down(false); // тело if (строка 2) перепрыгивается → 3 "}"
        assert_eq!(t.cursor_line(), 3);
        // Не-заголовок — no-op.
        t.toggle_fold(2);
        assert!(t.doc.line_meta().is_folded(1));
        // Fold all: оба блока.
        t.fold_all();
        assert!(t.doc.line_meta().is_folded(0));
        assert!(t.doc.line_meta().is_folded(1));
        // Unfold all.
        t.unfold_all();
        assert!(!t.doc.line_meta().is_folded(0));
        assert!(!t.doc.line_meta().is_folded(1));
        // Свёрнутое состояние переживает undo правки (флаги в LineMeta).
        t.toggle_fold(0);
        t.set_caret_at(6, 5);
        t.insert_text("!");
        assert!(t.undo());
        assert!(t.doc.line_meta().is_folded(0), "fold survives undo");
    }

    /// 4.6, сценарий «Сортировка выделения»: выделенные строки
    /// перестраиваются, остальной текст не меняется; undo откатывает.
    #[test]
    fn line_ops_sort_selection_scenario() {
        let mut t = BufferTab::from_text("header\npear\napple\nbanana\nfooter");
        // Выделение строк 1..=3: от (1,0) до (3,6) — конец в середине.
        let s = t.doc.line_to_char(1);
        let e = t.doc.line_to_char(3) + 6;
        t.set_caret_at(1, 0);
        t.select_to(3, 6);
        assert_eq!((t.primary().start, t.primary().end), (s, e));
        t.sort_lines(true, false);
        assert_eq!(t.doc.text(), "header\napple\nbanana\npear\nfooter");
        // Undo через History — операция регистрировалась как правка.
        assert!(t.undo());
        assert_eq!(t.doc.text(), "header\npear\napple\nbanana\nfooter");
        // По убыванию, с учётом и без учёта регистра.
        let mut t = BufferTab::from_text("Beta\nalpha\nCharlie\n");
        t.sort_lines(false, false); // весь файл, байтовый порядок по убыванию
        assert_eq!(t.doc.text(), "alpha\nCharlie\nBeta\n");
        let mut t = BufferTab::from_text("Beta\nalpha\nCharlie\n");
        t.sort_lines(true, true); // ignore case
        assert_eq!(t.doc.text(), "alpha\nBeta\nCharlie\n");
        let mut t = BufferTab::from_text("Beta\nalpha\nCharlie\n");
        t.sort_lines(true, false); // байтовый порядок: 'B' < 'a'
        assert_eq!(t.doc.text(), "Beta\nCharlie\nalpha\n");
    }

    /// 4.6: scope строк из выделений — строки, пересекаемые выделением;
    /// конец в колонке 0 строку не захватывает; несколько выделений —
    /// объединение.
    #[test]
    fn line_ops_scope_from_selections() {
        let mut t = BufferTab::from_text("c\nb\na\nz\ny\nx");
        // Выделение на строках 1..=2 (от col1 строки 1 до col1 строки 2).
        t.set_caret_at(1, 1);
        t.select_to(2, 1); // конец в колонке 1 — строка 2 захватывается
        t.sort_lines(true, false);
        assert_eq!(t.doc.text(), "c\na\nb\nz\ny\nx");
        // Конец ровно в колонке 0: строка под концом не трогается.
        let mut t = BufferTab::from_text("c\nb\na\nz");
        t.set_caret_at(1, 0);
        t.select_to(2, 0); // [line1_start..line2_start) — только строка 1
        t.sort_lines(false, false);
        assert_eq!(t.doc.text(), "c\nb\na\nz", "sort of single line — no-op");
        // Два выделения: объединение диапазонов.
        let mut t = BufferTab::from_text("c\nb\na\nz\ny\nx");
        t.set_cursors(vec![
            Selection::range(t.doc.line_to_char(1), t.doc.line_to_char(2)),
            Selection::range(t.doc.line_to_char(4), t.doc.line_to_char(5)),
        ]);
        t.sort_lines(true, false); // строки 1..=4
        assert_eq!(t.doc.text(), "c\na\nb\ny\nz\nx");
    }

    /// 4.6, сценарий «Удаление дубликатов»: по одному экземпляру,
    /// порядок первых вхождений.
    #[test]
    fn line_ops_dedupe_scenario() {
        let mut t = BufferTab::from_text("b\na\nb\nc\na\nb\n");
        t.dedupe_lines();
        assert_eq!(t.doc.text(), "b\na\nc\n");
        assert!(t.undo());
        assert_eq!(t.doc.text(), "b\na\nb\nc\na\nb\n");
        // На выделении — только выделенные строки.
        let mut t = BufferTab::from_text("keep\nb\nb\nb\nkeep");
        t.set_caret_at(1, 0);
        t.select_to(3, 1);
        t.dedupe_lines();
        assert_eq!(t.doc.text(), "keep\nb\nkeep");
    }

    /// 4.6: удаление пустых строк (выделение/весь файл).
    #[test]
    fn line_ops_remove_empty() {
        let mut t = BufferTab::from_text("a\n\nb\n\n\nc\n");
        t.remove_empty_lines();
        assert_eq!(t.doc.text(), "a\nb\nc\n");
    }

    /// 4.6, сценарий «Верхний регистр»: буквы выделения заглавные,
    /// длина и остальной текст не меняются; прочие режимы регистра.
    #[test]
    fn line_ops_change_case() {
        let mut t = BufferTab::from_text("say hello, world! ok");
        t.set_caret_at(0, 4);
        t.select_to(0, 17); // "hello, world!"
        t.change_case(editor_core::CaseMode::Upper);
        assert_eq!(t.doc.text(), "say HELLO, WORLD! ok");
        assert_eq!(t.doc.len_chars(), 20, "длина не меняется");
        // Нижний.
        t.change_case(editor_core::CaseMode::Lower); // на том же выделении
        assert_eq!(t.doc.text(), "say hello, world! ok");
        // Без выделения — весь файл.
        let mut t = BufferTab::from_text("one Two\nthree FOUR\n");
        t.change_case(editor_core::CaseMode::Upper);
        assert_eq!(t.doc.text(), "ONE TWO\nTHREE FOUR\n");
        // Title / Sentence — весь файл.
        let mut t = BufferTab::from_text("mixed case text here\n");
        t.change_case(editor_core::CaseMode::Title);
        assert_eq!(t.doc.text(), "Mixed Case Text Here\n");
        let mut t = BufferTab::from_text("first one. second one! THIRD?\n");
        t.change_case(editor_core::CaseMode::Sentence);
        assert_eq!(t.doc.text(), "First one. Second one! Third?\n");
        // Undo работает для регистра.
        assert!(t.undo());
        assert_eq!(t.doc.text(), "first one. second one! THIRD?\n");
    }

    /// Ввод группируется: серия одиночных вводов — один undo.
    #[test]
    fn typing_grouped_into_single_undo() {
        let mut t = BufferTab::untitled();
        for ch in "abc".chars() {
            t.insert_text(&ch.to_string());
        }
        assert!(t.undo());
        assert_eq!(t.doc.text(), "");
        assert!(!t.undo());
    }

    /// Backspace / forward delete / enter.
    #[test]
    fn backspace_delete_enter() {
        let mut t = BufferTab::untitled();
        t.insert_text("ab");
        t.backspace(); // удаляет 'b'
        assert_eq!(t.doc.text(), "a");
        t.insert_text("\n");
        assert_eq!(t.doc.text(), "a\n");
        t.set_caret_at(0, 0);
        t.delete_forward(); // удаляет 'a'
        assert_eq!(t.doc.text(), "\n");

        // Границы: backspace в 0 и delete в конце — no-op. End ставит
        // каретку перед терминатором строки, поэтому для no-op delete
        // каретку в конец документа ставим явно.
        t.move_home(false);
        let before = t.doc.text();
        t.backspace();
        assert_eq!(t.doc.text(), before);
        t.set_caret_at(t.doc.len_lines() - 1, t.line_len(t.doc.len_lines() - 1));
        t.delete_forward();
        assert_eq!(t.doc.text(), before);
    }

    /// Навигация: стрелки/Home/End по многострочному документу,
    /// включая clamp столбца на короткой строке.
    #[test]
    fn cursor_movement() {
        let mut t = BufferTab::from_text("one\nthree\nxx\n");
        // "one"=3 chars, line0; "three"=5, line1; "xx"=2, line2.
        t.set_caret_at(1, 3); // line1, col3

        t.move_home(false);
        assert_eq!((t.cursor_line(), t.cursor_col()), (1, 0));
        t.move_end(false);
        assert_eq!((t.cursor_line(), t.cursor_col()), (1, 5));

        t.move_up(false); // col 5 -> clamp на "one" (len 3)
        assert_eq!((t.cursor_line(), t.cursor_col()), (0, 3));
        t.move_up(false); // clamp на строку 0
        assert_eq!(t.cursor_line(), 0);
        t.move_down(false);
        t.move_down(false); // на "xx"
        t.move_end(false);
        assert_eq!((t.cursor_line(), t.cursor_col()), (2, 2));
        t.move_left(false);
        assert_eq!(t.cursor_col(), 1);
        t.move_right(false);
        t.move_right(false); // clamp в конец документа
        assert_eq!(t.primary().head(), t.doc.len_chars());
    }

    /// 4.2: goal column (D9.2) — вертикальное движение помнит столбец,
    /// горизонтальное движение и клик его сбрасывают.
    #[test]
    fn goal_column_vertical_navigation() {
        let mut t = BufferTab::from_text("longline10\nhi\nmidline\n");
        // Строки: 0="longline10"(10), 1="hi"(2), 2="midline"(7).
        t.set_caret_at(0, 10); // конец строки 0, goal = None
        t.move_down(false); // clamp до 2, goal := 10
        assert_eq!((t.cursor_line(), t.cursor_col()), (1, 2));
        t.move_down(false); // goal 10 → clamp до 7 на "midline"
        assert_eq!((t.cursor_line(), t.cursor_col()), (2, 7));
        t.move_up(false); // goal 10 → clamp до 2 на "hi"
        assert_eq!((t.cursor_line(), t.cursor_col()), (1, 2));
        // Горизонтальное движение сбрасывает goal: было (1,2), после
        // move_left голова в col 1, вверх даст col 1.
        t.move_left(false);
        t.move_up(false);
        assert_eq!((t.cursor_line(), t.cursor_col()), (0, 1));
        // Клик тоже сбрасывает.
        t.set_caret_at(0, 10);
        t.move_down(false);
        t.set_caret_at(2, 0);
        t.move_up(false);
        assert_eq!((t.cursor_line(), t.cursor_col()), (1, 0));
    }

    /// 4.2: PgUp/PgDn — страница = переданное число строк, clamp по
    /// границам, столбец сохраняется. Документ: 5 строк + пустая
    /// хвостовая (терминатор в конце — отдельная строка 5).
    #[test]
    fn move_page_navigation() {
        let mut t = BufferTab::from_text("abcdef\nx\nyy\nzzz\nwwww\n");
        t.set_caret_at(0, 3);
        t.move_page(1, 3, false); // строка 0 + 3 → строка 3
        assert_eq!((t.cursor_line(), t.cursor_col()), (3, 3));
        t.move_page(10, 3, false); // clamp в последнюю (пустую хвостовую)
        assert_eq!((t.cursor_line(), t.cursor_col()), (5, 0));
        t.move_page(-1, 3, false); // goal 3 → clamp до 2 на "yy"
        assert_eq!((t.cursor_line(), t.cursor_col()), (2, 2));
        t.move_page(-10, 3, false); // clamp в первую
        assert_eq!((t.cursor_line(), t.cursor_col()), (0, 3));
    }

    /// 4.2: клик → каретка; shift-клик → расширение выделения от якоря.
    #[test]
    fn click_and_select_to() {
        // "alpha\nbeta\ngamma\n": line1 стартует с 6, line2 — с 11.
        let mut t = BufferTab::from_text("alpha\nbeta\ngamma\n");
        t.set_caret_at(1, 2); // 't' в "beta", позиция 8
        assert_eq!(t.primary().head(), 8);
        // Клик дальше конца строки — clamp в её длину.
        t.set_caret_at(1, 99);
        assert_eq!((t.cursor_line(), t.cursor_col()), (1, 4));
        // Строка вне документа — clamp в последнюю.
        t.set_caret_at(99, 1);
        assert_eq!((t.cursor_line(), t.cursor_col()), (3, 0));
        // Расширение: якорь 11 (line2 col0), голова 10 (line1 col4) —
        // reversed [10..11).
        t.set_caret_at(2, 0);
        t.select_to(1, 4);
        assert_eq!(*t.primary(), Selection::range(11, 10));
        assert!(t.primary().reversed);
        // Расширение дальше влево: якорь 11 сохраняется, голова 1 →
        // [1..11), голова слева.
        t.select_to(0, 1);
        assert_eq!(*t.primary(), Selection::range(11, 1));
        assert!(t.primary().reversed);
        assert_eq!(t.primary().head(), 1);
    }

    /// 4.2: shift-стрелки расширяют выделение главной каретки.
    #[test]
    fn shift_arrows_extend_selection() {
        let mut t = BufferTab::from_text("abcdef");
        t.set_caret_at(0, 1);
        t.move_right(true);
        t.move_right(true);
        assert_eq!(*t.primary(), Selection::range(1, 3));
        t.move_left(true); // голова назад: [1..2), голова всё ещё справа
        assert_eq!(*t.primary(), Selection::range(1, 2));
        assert!(!t.primary().reversed);
        t.move_left(true); // голова = якорю: схлопывается
        assert_eq!(*t.primary(), Selection::range(1, 1));
        assert!(!t.primary().reversed);
        t.move_left(true); // голова левее якоря — reversed [0..1)
        assert_eq!(*t.primary(), Selection::range(1, 0));
        assert!(t.primary().reversed);
        // Движение без shift сбрасывает выделение в каретку (в голове).
        t.move_right(false);
        assert_eq!(*t.primary(), Selection::caret(1));
    }

    /// Загрузка файла: dirty=false, история пуста, каретка в начале.
    #[test]
    fn opened_file_starts_clean() {
        let dir = TempDir::new("clean-open");
        let a = dir.write("a.txt", "line\n");
        let mut m = TabModel::new();
        m.open_path(&a).unwrap();
        let t = m.active().unwrap();
        assert!(!t.dirty);
        assert!(!t.history.can_undo());
        assert_eq!(*t.primary(), Selection::caret(0));
        assert_eq!(t.doc.text(), "line\n");
    }

    #[test]
    fn title_is_file_name() {
        let dir = TempDir::new("title");
        let nested = dir.path().join("sub");
        std::fs::create_dir_all(&nested).unwrap();
        let p = nested.join("deep.rs");
        std::fs::write(&p, "fn main() {}").unwrap();
        let mut m = TabModel::new();
        m.open_path(&p).unwrap();
        assert_eq!(m.title(0).unwrap(), "deep.rs");
    }

    impl BufferTab {
        /// Хелпер тестов: документ из текста (координаты как у from_str).
        fn from_text(s: &str) -> Self {
            let mut t = BufferTab::untitled();
            t.doc = Document::from_str(s);
            t
        }
    }
}
