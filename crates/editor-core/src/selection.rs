//! Модель выделения: диапазон с направлением и каретки.
//! Множественные курсоры — срез `&[Selection]`; транзакции строятся
//! над всеми выделениями сразу (пересекающиеся отбрасывает
//! `Transaction::from_edits`).

use crate::transaction::{Edit, Transaction};

/// Выделение: `[start, end)` в char-индексах, `reversed` — направление
/// (голова слева от якоря). Каретка — вырожденное выделение `start == end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub start: usize,
    pub end: usize,
    pub reversed: bool,
}

impl Selection {
    /// Каретка в позиции `pos`.
    pub fn caret(pos: usize) -> Self {
        Selection { start: pos, end: pos, reversed: false }
    }

    /// Выделение между `a` и `b` в любом порядке; `reversed` отражает,
    /// какая точка была якорем, а какая — головой.
    pub fn range(a: usize, b: usize) -> Self {
        if a <= b {
            Selection { start: a, end: b, reversed: false }
        } else {
            Selection { start: b, end: a, reversed: true }
        }
    }

    pub fn is_caret(&self) -> bool {
        self.start == self.end
    }

    /// Голова (движущийся конец выделения).
    pub fn head(&self) -> usize {
        if self.reversed { self.start } else { self.end }
    }

    /// Якорь (неподвижный конец выделения).
    pub fn anchor(&self) -> usize {
        if self.reversed { self.end } else { self.start }
    }
}

/// Ввод текста во все выделения: каретка → вставка в позицию,
/// диапазон → замена выделенного текста (как при вводе поверх выделения).
pub fn insert_text(sels: &[Selection], text: &str) -> Transaction {
    let edits = sels
        .iter()
        .filter(|s| !text.is_empty() || !s.is_caret())
        .map(|s| Edit { start: s.start, end: s.end, text: text.to_string() })
        .collect();
    Transaction::from_edits(edits)
}

/// Backspace: удаляет выделенный диапазон целиком либо один символ
/// перед кареткой. Каретка в начале документа — no-op.
/// Документ LF-нормализован (D6), поэтому символ — ровно одна char-единица.
pub fn backspace(_len_chars: usize, sels: &[Selection]) -> Transaction {
    let edits = sels
        .iter()
        .filter_map(|s| {
            if !s.is_caret() {
                Some(Edit::delete(s.start, s.end))
            } else if s.start > 0 {
                Some(Edit::delete(s.start - 1, s.start))
            } else {
                None
            }
        })
        .collect();
    Transaction::from_edits(edits)
}

/// Forward delete: удаляет выделенный диапазон целиком либо один символ
/// после каретки. Каретка в конце документа — no-op.
pub fn forward_delete(len_chars: usize, sels: &[Selection]) -> Transaction {
    let edits = sels
        .iter()
        .filter_map(|s| {
            if !s.is_caret() {
                Some(Edit::delete(s.start, s.end))
            } else if s.start < len_chars {
                Some(Edit::delete(s.start, s.start + 1))
            } else {
                None
            }
        })
        .collect();
    Transaction::from_edits(edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Обязательный тест задачи 2.2: ввод текста (запятая) на 3 каретках.
    #[test]
    fn insert_text_on_three_carets() {
        let sels = vec![Selection::caret(3), Selection::caret(7), Selection::caret(13)];
        let mut doc = crate::document::Document::from_str("one two three");
        let tx = insert_text(&sels, ",");
        let inverse = doc.apply(&tx);
        assert_eq!(doc.text(), "one, two, three,");
        // Все каретки сместились за вставленный текст.
        let mapped: Vec<Selection> = sels.iter().map(|s| tx.map_selection(s)).collect();
        assert_eq!(mapped, vec![Selection::caret(4), Selection::caret(9), Selection::caret(16)]);
        // Инверсия возвращает исходный текст.
        doc.apply(&inverse);
        assert_eq!(doc.text(), "one two three");
    }

    #[test]
    fn insert_text_over_three_selections() {
        // Ввод поверх выделений: каждое слово заменяется на "X".
        let sels =
            vec![Selection::range(0, 3), Selection::range(4, 7), Selection::range(8, 13)];
        let mut doc = crate::document::Document::from_str("one two three");
        let tx = insert_text(&sels, "X");
        doc.apply(&tx);
        assert_eq!(doc.text(), "X X X");
        // По заданной политике map_selection диапазон растягивается вокруг
        // вставки: остаётся выделением поверх введённого текста.
        let mapped: Vec<Selection> = sels.iter().map(|s| tx.map_selection(s)).collect();
        assert_eq!(
            mapped,
            vec![
                Selection::range(0, 1),
                Selection::range(2, 3),
                Selection::range(4, 5)
            ]
        );
    }

    #[test]
    fn caret_helpers() {
        let c = Selection::caret(5);
        assert!(c.is_caret());
        assert_eq!(c.head(), 5);
        assert_eq!(c.anchor(), 5);

        let r = Selection::range(7, 3);
        assert!(!r.is_caret());
        assert_eq!((r.start, r.end, r.reversed), (3, 7, true));
        // У reversed-выделения голова слева (пользователь тянул справа налево).
        assert_eq!(r.head(), 3);
        assert_eq!(r.anchor(), 7);

        let f = Selection::range(3, 7);
        assert_eq!((f.start, f.end, f.reversed), (3, 7, false));
        assert_eq!(f.head(), 7);
        assert_eq!(f.anchor(), 3);
    }

    #[test]
    fn backspace_on_carets_and_ranges() {
        let mut doc = crate::document::Document::from_str("abcdef");
        // Каретки в 1 и 5 (удаляют 'a' и 'e'), диапазон [3..4) (удаляет 'd').
        let sels = vec![Selection::caret(1), Selection::range(3, 4), Selection::caret(5)];
        let tx = backspace(doc.len_chars(), &sels);
        let inverse = doc.apply(&tx);
        assert_eq!(doc.text(), "bcf");
        let mapped: Vec<Selection> = sels.iter().map(|s| tx.map_selection(s)).collect();
        assert_eq!(mapped, vec![Selection::caret(0), Selection::caret(2), Selection::caret(2)]);
        doc.apply(&inverse);
        assert_eq!(doc.text(), "abcdef");
    }

    #[test]
    fn backspace_at_zero_is_noop() {
        let tx = backspace(3, &[Selection::caret(0)]);
        assert!(tx.is_empty());
    }

    #[test]
    fn forward_delete_on_carets_and_ranges() {
        let mut doc = crate::document::Document::from_str("abcdef");
        // Каретка в 0 (удаляет 'a'), диапазон [2..4) (удаляет "cd"),
        // каретка в 6 — конец документа, no-op.
        let sels = vec![Selection::caret(0), Selection::range(2, 4), Selection::caret(6)];
        let tx = forward_delete(doc.len_chars(), &sels);
        doc.apply(&tx);
        assert_eq!(doc.text(), "bef");
    }

    #[test]
    fn forward_delete_at_end_is_noop() {
        let tx = forward_delete(3, &[Selection::caret(3)]);
        assert!(tx.is_empty());
    }

    /// Пересекающиеся выделения: остаётся первая правка, вторая отброшена.
    #[test]
    fn overlapping_selections_dropped() {
        let sels = vec![Selection::range(0, 5), Selection::range(3, 8)];
        let tx = insert_text(&sels, "X");
        assert_eq!(tx.edits().len(), 1);
        let mut doc = crate::document::Document::from_str("abcdefghij");
        doc.apply(&tx);
        assert_eq!(doc.text(), "Xfghij");
    }

    /// Направление выделения сохраняется при маппинге и не влияет на ввод.
    #[test]
    fn reversed_selection_semantics() {
        let sel = Selection::range(7, 3);
        let mut doc = crate::document::Document::from_str("abcdefg");
        let tx = insert_text(&[sel], "XY");
        doc.apply(&tx);
        assert_eq!(doc.text(), "abcXY");
        // Голова (3, левый конец) осталась на месте, якорь (7) ушёл за
        // вставку; reversed сохранён.
        let mapped = tx.map_selection(&sel);
        assert!(mapped.reversed);
        assert_eq!(mapped.head(), 3);
        assert_eq!(mapped.anchor(), 5);
        assert_eq!(mapped, Selection { start: 3, end: 5, reversed: true });

        // Вырожденный после удаления диапазон превращается в каретку,
        // направление формально сохраняется.
        let tx2 = crate::transaction::Transaction::from_edits(vec![crate::transaction::Edit::delete(0, 7)]);
        let mapped2 = tx2.map_selection(&sel);
        assert!(mapped2.is_caret());
        assert_eq!(mapped2.start, 0);
        assert!(mapped2.reversed);
    }
}
