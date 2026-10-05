//! Undo/Redo поверх транзакций (задача 2.4).
//!
//! `History` хранит два стека групп. Группа — упорядоченный вектор
//! транзакций-инверсий одного «сеанса ввода»: запись undo-стека
//! определена в координатах состояния документа сразу после своей
//! правки и откатывает её — именно такую инверсию возвращает
//! `Document::apply`. `undo` применяет записи группы от новой к старой;
//! каждый шаг возвращает прямую транзакцию (в координатах состояния
//! до шага) — из них собирается группа redo-стека; `redo` симметричен.
//! Каждый шаг — корректная пара apply/inverse (инвариант
//! `inverse.apply == original` из transaction.rs), поэтому схема точна
//! без пересчёта координат; LineMeta синхронизируется автоматически
//! внутри `Document::apply` — закладки и folded переживают undo/redo.
//!
//! Группировка последовательного ввода (design.md D2.4): подряд идущие
//! транзакции одного типа — все правки одиночные вставки символов
//! (кареток может быть несколько) либо все одиночные удаления —
//! образуют одну запись undo. Таймауты и позиционные эвристики не
//! используются — только «подряд по типу»; правка другого типа,
//! составная правка (замена, вставка/удаление нескольких символов)
//! или undo/redo закрывают группу. Попытка склеивать инверсии в одну
//! транзакцию пересчётом координат отвергнута: для групп с
//! немонотонным движением каретки (forward-delete, ввод внутри
//! уже введённого) порядок вставок в одной позиции неоднозначен —
//! см. D2.4.

use crate::document::Document;
use crate::transaction::Transaction;

/// Тип группы последовательного ввода.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GroupKind {
    /// Одиночные вставки символов (возможно на нескольких каретках).
    Insert,
    /// Одиночные удаления одного символа.
    Delete,
}

/// История правок документа: undo/redo стеки с группировкой
/// последовательного ввода.
#[derive(Debug, Default)]
pub struct History {
    /// Группы инверсных транзакций; внутри группы — от старой к новой.
    undo_stack: Vec<Vec<Transaction>>,
    /// Группы прямых транзакций; внутри — в порядке применения.
    redo_stack: Vec<Vec<Transaction>>,
    /// Тип текущей открытой группы; `None` — следующая правка начинает
    /// новую группу.
    group: Option<GroupKind>,
}

impl History {
    pub fn new() -> Self {
        History::default()
    }

    /// Применяет транзакцию к документу и регистрирует её в истории.
    pub fn apply(&mut self, doc: &mut Document, tx: &Transaction) {
        if tx.is_empty() {
            return;
        }
        let inv = doc.apply(tx);
        self.push(tx.clone(), inv);
    }

    /// Регистрирует уже применённую вручную транзакцию: `inverse` —
    /// результат `Document::apply` (координаты текущего состояния).
    pub fn push(&mut self, tx: Transaction, inverse: Transaction) {
        if tx.is_empty() {
            return;
        }
        self.redo_stack.clear();
        let kind = group_kind(&tx);
        let joins_group =
            kind.is_some() && self.group == kind && self.undo_stack.last().is_some();
        if joins_group {
            self.undo_stack.last_mut().expect("проверено выше").push(inverse);
        } else {
            self.undo_stack.push(vec![inverse]);
        }
        self.group = kind;
    }

    /// Откатывает последнюю группу правок. `false` — стек пуст, no-op.
    pub fn undo(&mut self, doc: &mut Document) -> bool {
        let Some(group) = self.undo_stack.pop() else {
            return false;
        };
        // Инверсии применяются от новой к старой; каждая возвращает
        // прямую транзакцию для redo — собираем их в порядок применения.
        let mut redo_group: Vec<Transaction> = Vec::with_capacity(group.len());
        for entry in group.into_iter().rev() {
            redo_group.push(doc.apply(&entry));
        }
        redo_group.reverse();
        self.redo_stack.push(redo_group);
        self.group = None;
        true
    }

    /// Повторяет последнюю отменённую группу правок. `false` — стек
    /// пуст, no-op.
    pub fn redo(&mut self, doc: &mut Document) -> bool {
        let Some(group) = self.redo_stack.pop() else {
            return false;
        };
        // Прямые транзакции применяются по порядку; каждая возвращает
        // инверсию (в координатах состояния после шага) — это новая
        // undo-группа, упорядоченная от старой к новой.
        let mut undo_group: Vec<Transaction> = Vec::with_capacity(group.len());
        for entry in &group {
            undo_group.push(doc.apply(entry));
        }
        self.undo_stack.push(undo_group);
        self.group = None;
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }
}

/// Классификация транзакции для группировки: `Some(Insert)` — все правки
/// одиночные чистые вставки, `Some(Delete)` — все правки удаления ровно
/// одного символа, `None` — составная правка (своих групп не образует
/// и чужие не продолжает).
fn group_kind(tx: &Transaction) -> Option<GroupKind> {
    let edits = tx.edits();
    if edits.is_empty() {
        return None;
    }
    if edits.iter().all(|e| e.start == e.end && e.text.chars().count() == 1) {
        return Some(GroupKind::Insert);
    }
    if edits.iter().all(|e| e.start + 1 == e.end && e.text.is_empty()) {
        return Some(GroupKind::Delete);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::{Selection, backspace, forward_delete, insert_text};
    use crate::transaction::Edit;

    /// Обязательный тест 2.4: серия разнотипных правок; undo возвращает
    /// документ к исходному байтовому состоянию.
    #[test]
    fn undo_restores_original_byte_state() {
        let original = "hello world";
        let mut doc = Document::from_text(original);
        let mut hist = History::new();

        hist.apply(&mut doc, &Transaction::from_edits(vec![Edit::insert(5, " beautiful")]));
        assert_eq!(doc.text(), "hello beautiful world");
        hist.apply(&mut doc, &Transaction::from_edits(vec![Edit::delete(0, 6)]));
        assert_eq!(doc.text(), "beautiful world");
        hist.apply(
            &mut doc,
            &Transaction::from_edits(vec![Edit { start: 10, end: 15, text: "there".into() }]),
        );
        assert_eq!(doc.text(), "beautiful there");
        hist.apply(
            &mut doc,
            &Transaction::from_edits(vec![Edit::insert(0, "<"), Edit::insert(15, ">")]),
        );
        assert_eq!(doc.text(), "<beautiful there>");

        for i in 0..4 {
            assert!(hist.undo(&mut doc), "undo {i}");
        }
        assert_eq!(doc.text().as_bytes(), original.as_bytes());
        assert!(!hist.undo(&mut doc));
    }

    /// Redo возвращает состояние после правок.
    #[test]
    fn redo_restores_after_undo() {
        let mut doc = Document::from_text("текст");
        let mut hist = History::new();
        hist.apply(&mut doc, &Transaction::from_edits(vec![Edit::insert(0, ">> ")]));
        hist.apply(&mut doc, &Transaction::from_edits(vec![Edit::delete(0, 3)]));
        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), ">> текст");
        assert!(hist.can_redo());
        assert!(hist.redo(&mut doc));
        assert_eq!(doc.text(), "текст");
        assert!(!hist.can_redo());
    }

    /// N одиночных вводов подряд склеиваются в один undo (в т.ч.
    /// многобайтовые символы — координаты честные char-индексы).
    #[test]
    fn consecutive_typing_forms_single_undo() {
        let mut doc = Document::from_text("");
        let mut hist = History::new();
        for (caret, ch) in "Привет, мир!".chars().enumerate() {
            hist.apply(&mut doc, &insert_text(&[Selection::caret(caret)], &ch.to_string()));
        }
        assert_eq!(doc.text(), "Привет, мир!");

        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text().as_bytes(), b"");
        assert!(!hist.can_undo());

        assert!(hist.redo(&mut doc));
        assert_eq!(doc.text(), "Привет, мир!");
    }

    /// Мульти-курсорный ввод тоже группируется: два раунда ввода запятых
    /// на двух каретках отменяются одним undo.
    #[test]
    fn multi_cursor_typing_groups() {
        let mut doc = Document::from_text("one two");
        let mut hist = History::new();
        for _ in 0..2 {
            let carets = [Selection::caret(3), Selection::caret(doc.len_chars())];
            hist.apply(&mut doc, &insert_text(&carets, ","));
        }
        assert_eq!(doc.text(), "one,, two,,");
        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "one two");
    }

    /// Последовательные одиночные backspace — одна группа.
    #[test]
    fn consecutive_backspaces_form_single_undo() {
        let mut doc = Document::from_text("abcdef");
        let mut hist = History::new();
        for caret in (1..=6).rev() {
            let tx = backspace(doc.len_chars(), &[Selection::caret(caret)]);
            hist.apply(&mut doc, &tx);
        }
        assert_eq!(doc.text(), "");
        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "abcdef");
    }

    /// Последовательные forward delete — одна группа (контрпример,
    /// ломавший покоординатное слияние инверсий).
    #[test]
    fn consecutive_forward_deletes_form_single_undo() {
        let mut doc = Document::from_text("abcdef");
        let mut hist = History::new();
        for _ in 0..3 {
            let tx = forward_delete(doc.len_chars(), &[Selection::caret(0)]);
            hist.apply(&mut doc, &tx);
        }
        assert_eq!(doc.text(), "def");
        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "abcdef");
        assert!(hist.redo(&mut doc));
        assert_eq!(doc.text(), "def");
    }

    /// Ввод внутри уже введённого текста группы (каретка вернулась) —
    /// один undo восстанавливает порядок символов.
    #[test]
    fn typing_with_caret_back_inside_group_undoes_correctly() {
        let mut doc = Document::from_text("");
        let mut hist = History::new();
        hist.apply(&mut doc, &insert_text(&[Selection::caret(0)], "a"));
        hist.apply(&mut doc, &insert_text(&[Selection::caret(1)], "b"));
        hist.apply(&mut doc, &insert_text(&[Selection::caret(1)], "X")); // между a и b
        hist.apply(&mut doc, &insert_text(&[Selection::caret(3)], "c"));
        assert_eq!(doc.text(), "aXbc");
        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "");
        assert!(hist.redo(&mut doc));
        assert_eq!(doc.text(), "aXbc");
    }

    /// Правка другого типа разрывает группу: undo снимает сначала её.
    #[test]
    fn different_edit_kind_splits_groups() {
        let mut doc = Document::from_text("");
        let mut hist = History::new();
        hist.apply(&mut doc, &insert_text(&[Selection::caret(0)], "a"));
        hist.apply(&mut doc, &insert_text(&[Selection::caret(1)], "b"));
        let tx = backspace(doc.len_chars(), &[Selection::caret(2)]);
        hist.apply(&mut doc, &tx);
        assert_eq!(doc.text(), "a");

        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "ab"); // снят только backspace
        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), ""); // снята вся группа ввода
        assert!(!hist.undo(&mut doc));
    }

    /// Составная правка (вставка нескольких символов) — своя запись.
    #[test]
    fn multi_char_insert_is_own_undo_step() {
        let mut doc = Document::from_text("");
        let mut hist = History::new();
        hist.apply(&mut doc, &insert_text(&[Selection::caret(0)], "a"));
        hist.apply(&mut doc, &insert_text(&[Selection::caret(1)], "b"));
        hist.apply(&mut doc, &insert_text(&[Selection::caret(2)], "XYZ"));
        hist.apply(&mut doc, &insert_text(&[Selection::caret(5)], "c"));
        assert_eq!(doc.text(), "abXYZc");

        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "abXYZ");
        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "ab");
        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "");
    }

    /// Undo/redo на пустых стеках — no-op (false), документ не меняется.
    #[test]
    fn undo_redo_on_empty_stacks_are_noops() {
        let mut doc = Document::from_text("текст");
        let mut hist = History::new();
        assert!(!hist.undo(&mut doc));
        assert!(!hist.redo(&mut doc));
        assert_eq!(doc.text(), "текст");
        assert!(!hist.can_undo());
        assert!(!hist.can_redo());

        // Пустая транзакция ничего не регистрирует.
        hist.apply(&mut doc, &backspace(5, &[Selection::caret(0)]));
        assert!(!hist.can_undo());
    }

    /// Мульти-курсорные правки из selection.rs (insert_text и backspace)
    /// отменяются одним undo.
    #[test]
    fn multi_cursor_edit_undone_with_single_undo() {
        let mut doc = Document::from_text("one two three");
        let mut hist = History::new();
        let sels = [Selection::caret(3), Selection::caret(7), Selection::caret(13)];
        hist.apply(&mut doc, &insert_text(&sels, ","));
        assert_eq!(doc.text(), "one, two, three,");
        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "one two three");

        let sels = [Selection::caret(2), Selection::caret(6)];
        let tx = backspace(doc.len_chars(), &sels);
        hist.apply(&mut doc, &tx);
        assert_eq!(doc.text(), "oe to three");
        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "one two three");
    }

    /// Закладки и folded переживают undo/redo (LineMeta синхронизируется
    /// внутри Document::apply — и при прямых, и при инверсных транзакциях).
    #[test]
    fn bookmarks_and_folds_survive_undo_redo() {
        let mut doc = Document::from_text("l0\nl1\nl2\nl3\n");
        {
            let meta = doc.line_meta_mut();
            meta.set_bookmark(2);
            meta.set_folded(3, true);
        }
        let mut hist = History::new();
        hist.apply(&mut doc, &Transaction::from_edits(vec![Edit::insert(0, "x\ny\n")]));
        assert_eq!(doc.text(), "x\ny\nl0\nl1\nl2\nl3\n");
        assert_eq!(doc.line_meta().bookmarked_lines(), vec![4]);
        assert_eq!(doc.line_meta().folded_lines(), vec![5]);

        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "l0\nl1\nl2\nl3\n");
        assert_eq!(doc.line_meta().line_count(), doc.len_lines());
        assert_eq!(doc.line_meta().bookmarked_lines(), vec![2]);
        assert_eq!(doc.line_meta().folded_lines(), vec![3]);

        assert!(hist.redo(&mut doc));
        assert_eq!(doc.line_meta().bookmarked_lines(), vec![4]);
        assert_eq!(doc.line_meta().folded_lines(), vec![5]);
    }

    /// Новая правка после undo очищает redo-стек.
    #[test]
    fn new_edit_clears_redo_stack() {
        let mut doc = Document::from_text("");
        let mut hist = History::new();
        hist.apply(&mut doc, &insert_text(&[Selection::caret(0)], "a"));
        assert!(hist.undo(&mut doc));
        assert!(hist.can_redo());
        hist.apply(&mut doc, &insert_text(&[Selection::caret(0)], "b"));
        assert_eq!(doc.text(), "b");
        assert!(!hist.can_redo());
        assert!(!hist.redo(&mut doc));
        assert_eq!(doc.text(), "b");
    }

    /// Undo разрывает цепочку группировки: правки до и после undo —
    /// отдельные группы.
    #[test]
    fn group_does_not_span_undo() {
        let mut doc = Document::from_text("");
        let mut hist = History::new();
        hist.apply(&mut doc, &insert_text(&[Selection::caret(0)], "a"));
        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "");
        hist.apply(&mut doc, &insert_text(&[Selection::caret(0)], "b"));
        assert_eq!(doc.text(), "b");
        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "");
        assert!(!hist.can_undo());
    }

    /// Перемешанные undo/redo: undo, undo → redo, redo возвращает
    /// состояние после правок.
    #[test]
    fn interleaved_undo_redo_roundtrip() {
        let mut doc = Document::from_text("one");
        let mut hist = History::new();
        hist.apply(&mut doc, &insert_text(&[Selection::caret(3)], " two"));
        let tx = backspace(doc.len_chars(), &[Selection::caret(7)]);
        hist.apply(&mut doc, &tx);
        assert_eq!(doc.text(), "one tw");

        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "one two");
        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "one");
        assert!(hist.redo(&mut doc));
        assert_eq!(doc.text(), "one two");
        assert!(hist.redo(&mut doc));
        assert_eq!(doc.text(), "one tw");
        assert!(!hist.can_redo());
    }

    /// push после ручного применения (документ применяет сам,
    /// историю регистрирует отдельно).
    #[test]
    fn push_after_manual_apply() {
        let mut doc = Document::from_text("abc");
        let mut hist = History::new();
        let tx = Transaction::from_edits(vec![Edit::insert(3, "def")]);
        let inv = doc.apply(&tx);
        hist.push(tx, inv);
        assert_eq!(doc.text(), "abcdef");
        assert!(hist.undo(&mut doc));
        assert_eq!(doc.text(), "abc");
        assert!(hist.redo(&mut doc));
        assert_eq!(doc.text(), "abcdef");
    }
}
