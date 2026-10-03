//! Атомарные правки и транзакции над ropey-`Rope`.

use ropey::Rope;

use crate::selection::Selection;

/// Смещение при маппинге позиции, лежащей внутри изменённого диапазона.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bias {
    Left,
    Right,
}

/// Одна атомарная правка: заменить диапазон символов `[start, end)` на `text`.
/// Позиции — char-индексы в состоянии документа **до** применения транзакции.
/// Пустой диапазон (`start == end`) означает чистую вставку, пустой `text` — чистое удаление.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

impl Edit {
    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Edit { start: at, end: at, text: text.into() }
    }

    pub fn delete(start: usize, end: usize) -> Self {
        assert!(end >= start, "Edit::delete: end < start");
        Edit { start, end, text: String::new() }
    }

    fn insert_len(&self) -> usize {
        self.text.chars().count()
    }
}

/// Батч атомарных правок. Правки отсортированы по возрастанию `start`
/// и не пересекаются; позиции заданы в координатах документа до применения.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Transaction {
    edits: Vec<Edit>,
}

impl Transaction {
    pub fn new() -> Self {
        Transaction { edits: Vec::new() }
    }

    /// Строит транзакцию из правок: сортирует и отбрасывает пересекающиеся
    /// (оставляя первую по порядку). Правки с одинаковым `start`-диапазоном
    /// вставки не пересекаются и сохраняются.
    pub fn from_edits(mut edits: Vec<Edit>) -> Self {
        edits.sort_by_key(|e| (e.start, e.end));
        let mut filtered: Vec<Edit> = Vec::with_capacity(edits.len());
        for e in edits {
            // Пересекающуюся правку отбрасываем; касание по границе
            // (e.start == last.end, в т.ч. чистые вставки) допустимо.
            if filtered.last().is_some_and(|last| e.start < last.end) {
                continue;
            }
            filtered.push(e);
        }
        Transaction { edits: filtered }
    }

    pub fn edits(&self) -> &[Edit] {
        &self.edits
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// Применяет транзакцию к rope. Возвращает инверсную транзакцию
    /// (позиции — в координатах документа **после** применения),
    /// применяемую к изменённому rope для отката.
    pub fn apply(&self, rope: &mut Rope) -> Transaction {
        // Применяем в обратном порядке: правки с большими позициями не
        // сдвигают позиции предыдущих, поэтому все координаты остаются валидными.
        let mut removed: Vec<String> = Vec::with_capacity(self.edits.len());
        for edit in self.edits.iter().rev() {
            let text =
                if edit.end > edit.start { rope.slice(edit.start..edit.end).to_string() } else { String::new() };
            if edit.end > edit.start {
                rope.remove(edit.start..edit.end);
            }
            if !edit.text.is_empty() {
                rope.insert(edit.start, &edit.text);
            }
            removed.push(text);
        }
        removed.reverse();

        // Инверсные правки в пост-координатах: кумулятивный сдвиг delta
        // учитывает все предыдущие (по возрастанию) правки.
        let mut edits = Vec::with_capacity(self.edits.len());
        let mut delta: i64 = 0;
        for (edit, removed_text) in self.edits.iter().zip(removed) {
            let start = (edit.start as i64 + delta) as usize;
            let ins = edit.insert_len() as i64;
            edits.push(Edit { start, end: start + ins as usize, text: removed_text });
            delta += ins - (edit.end - edit.start) as i64;
        }
        Transaction { edits }
    }

    /// Маппинг позиции из до-транзакционных в после-транзакционные координаты.
    pub fn map_position(&self, pos: usize, bias: Bias) -> usize {
        let mut delta: i64 = 0;
        for edit in &self.edits {
            if pos < edit.start {
                break;
            }
            if pos > edit.end {
                delta += edit.insert_len() as i64 - (edit.end - edit.start) as i64;
                continue;
            }
            // pos внутри [start, end]
            let ins = edit.insert_len() as i64;
            return match bias {
                Bias::Left => (edit.start as i64 + delta) as usize,
                Bias::Right => (edit.start as i64 + delta + ins) as usize,
            };
        }
        (pos as i64 + delta) as usize
    }

    /// Маппинг выделения через транзакцию. Каретка маппится с `Bias::Right`
    /// (смещается за вставленный текст); у диапазона начало — `Bias::Left`,
    /// конец — `Bias::Right`: выделение растягивается вокруг вставки
    /// и не теряется при удалениях. Направление (`reversed`) сохраняется.
    pub fn map_selection(&self, sel: &Selection) -> Selection {
        if sel.is_caret() {
            Selection::caret(self.map_position(sel.start, Bias::Right))
        } else {
            Selection {
                start: self.map_position(sel.start, Bias::Left),
                end: self.map_position(sel.end, Bias::Right),
                reversed: sel.reversed,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(s: &str) -> Rope {
        Rope::from_str(s)
    }

    #[test]
    fn insert_at_start() {
        let mut r = rope("world");
        let tx = Transaction::from_edits(vec![Edit::insert(0, "hello ")]);
        let inv = tx.apply(&mut r);
        assert_eq!(r.to_string(), "hello world");
        let mut r2 = r.clone();
        inv.apply(&mut r2);
        assert_eq!(r2.to_string(), "world");
    }

    #[test]
    fn insert_in_middle() {
        let mut r = rope("helo");
        let tx = Transaction::from_edits(vec![Edit::insert(2, "l")]);
        tx.apply(&mut r);
        assert_eq!(r.to_string(), "hello");
    }

    #[test]
    fn insert_at_end() {
        let mut r = rope("hello");
        let tx = Transaction::from_edits(vec![Edit::insert(5, "!")]);
        tx.apply(&mut r);
        assert_eq!(r.to_string(), "hello!");
    }

    #[test]
    fn delete_at_start() {
        let mut r = rope("hello");
        let tx = Transaction::from_edits(vec![Edit::delete(0, 2)]);
        tx.apply(&mut r);
        assert_eq!(r.to_string(), "llo");
    }

    #[test]
    fn delete_in_middle() {
        let mut r = rope("hello");
        let tx = Transaction::from_edits(vec![Edit::delete(2, 4)]);
        tx.apply(&mut r);
        assert_eq!(r.to_string(), "heo");
    }

    #[test]
    fn delete_at_end() {
        let mut r = rope("hello");
        let tx = Transaction::from_edits(vec![Edit::delete(3, 5)]);
        tx.apply(&mut r);
        assert_eq!(r.to_string(), "hel");
    }

    #[test]
    fn replace_is_insert_plus_delete() {
        let mut r = rope("hello world");
        let tx = Transaction::from_edits(vec![Edit { start: 0, end: 5, text: "goodbye".into() }]);
        let inv = tx.apply(&mut r);
        assert_eq!(r.to_string(), "goodbye world");
        inv.apply(&mut r);
        assert_eq!(r.to_string(), "hello world");
    }

    #[test]
    fn multi_edit_transaction() {
        let mut r = rope("abcdef");
        // Позиции — до транзакции: [0..1)->"X", [3..4)->"Y", вставка в конец.
        let tx = Transaction::from_edits(vec![
            Edit { start: 0, end: 1, text: "X".into() },
            Edit { start: 3, end: 4, text: "Y".into() },
            Edit::insert(6, "Z"),
        ]);
        let inv = tx.apply(&mut r);
        assert_eq!(r.to_string(), "XbcYefZ");
        inv.apply(&mut r);
        assert_eq!(r.to_string(), "abcdef");
    }

    #[test]
    fn inverse_of_inverse_is_original() {
        let mut r = rope("hello world");
        let tx = Transaction::from_edits(vec![
            Edit::insert(5, " beautiful"),
            Edit::delete(0, 1),
        ]);
        let inv = tx.apply(&mut r);
        let orig = inv.apply(&mut r);
        assert_eq!(orig, tx);
    }

    #[test]
    fn map_position_after_insert() {
        let tx = Transaction::from_edits(vec![Edit::insert(5, "abc")]);
        assert_eq!(tx.map_position(2, Bias::Right), 2);
        assert_eq!(tx.map_position(5, Bias::Right), 8);
        assert_eq!(tx.map_position(5, Bias::Left), 5);
        assert_eq!(tx.map_position(7, Bias::Right), 10);
    }

    #[test]
    fn map_position_after_delete() {
        let tx = Transaction::from_edits(vec![Edit::delete(3, 8)]);
        assert_eq!(tx.map_position(1, Bias::Right), 1);
        assert_eq!(tx.map_position(3, Bias::Left), 3);
        assert_eq!(tx.map_position(5, Bias::Left), 3);
        assert_eq!(tx.map_position(10, Bias::Right), 5);
    }

    /// Задача 2.2: каретка смещается за вставленный текст.
    #[test]
    fn map_selection_caret_shifts_past_insertion() {
        let tx = Transaction::from_edits(vec![Edit::insert(5, "abc")]);
        assert_eq!(tx.map_selection(&Selection::caret(5)), Selection::caret(8));
        assert_eq!(tx.map_selection(&Selection::caret(2)), Selection::caret(2));
    }

    /// Задача 2.2: диапазон растягивается вокруг вставки.
    #[test]
    fn map_selection_range_stretches_around_insertion() {
        let tx = Transaction::from_edits(vec![Edit::insert(7, "abc")]);
        assert_eq!(tx.map_selection(&Selection::range(5, 10)), Selection::range(5, 13));
        // Вставка ровно на границе начала: Left удерживает начало перед вставкой.
        assert_eq!(tx.map_selection(&Selection::range(7, 10)), Selection::range(7, 13));
    }

    /// Задача 2.2: диапазон не теряется при удалениях, направление сохраняется.
    #[test]
    fn map_selection_range_survives_deletion() {
        let tx = Transaction::from_edits(vec![Edit::delete(0, 7)]);
        let mapped = tx.map_selection(&Selection { start: 5, end: 10, reversed: true });
        assert_eq!(mapped, Selection { start: 0, end: 3, reversed: true });

        let tx2 = Transaction::from_edits(vec![Edit::delete(6, 8)]);
        let mapped2 = tx2.map_selection(&Selection::range(5, 10));
        assert_eq!(mapped2, Selection::range(5, 8));
    }
}
