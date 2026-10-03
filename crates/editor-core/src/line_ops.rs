//! Операции над строками документа (задача 4.6, design.md D9.6):
//! сортировка, удаление дубликатов/пустых строк, преобразования
//! регистра. Чистые функции `Document → Transaction` (координаты —
//! до транзакции); применение через `History` — undo работает из
//! коробки. Вынесено в editor-core (а не в app): логика не зависит
//! от GUI и покрывается табличными тестами.

use crate::document::Document;
use crate::transaction::{Edit, Transaction};

/// Режим преобразования регистра (спека text-editing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseMode {
    /// ВЕРХНИЙ.
    Upper,
    /// нижний.
    Lower,
    /// Заголовочный (Первые Буквы Слов).
    Title,
    /// Как в предложении (первая после границы предложения).
    Sentence,
}

/// Границы блока строк в char-координатах: `[начало первой,
/// конец последней С терминатором)`; для блока до конца файла без
/// хвостового перевода строки — `len_chars`.
fn block_span(doc: &Document, first: usize, last: usize) -> (usize, usize) {
    let start = doc.line_to_char(first);
    let end = if last + 1 < doc.len_lines() {
        doc.line_to_char(last + 1)
    } else {
        doc.len_chars()
    };
    (start, end)
}

/// Сортировка строк `[first..=last]` (включительно). `ignore_case` —
/// сравнение по нижнему регистру; сортировка стабильная (равные строки
/// сохраняют порядок).
pub fn sort_lines(
    doc: &Document,
    first: usize,
    last: usize,
    ascending: bool,
    ignore_case: bool,
) -> Transaction {
    if last <= first {
        return Transaction::new();
    }
    let mut lines: Vec<String> = (first..=last).map(|l| doc.line_content(l)).collect();
    if ignore_case {
        lines.sort_by_key(|l| l.to_lowercase());
    } else {
        lines.sort();
    }
    if !ascending {
        lines.reverse();
    }
    replace_block(doc, first, last, lines)
}

/// Удаление повторяющихся строк: остаётся ровно по одному экземпляру,
/// порядок первых вхождений сохраняется (сценарий specs/text-editing).
pub fn dedupe_lines(doc: &Document, first: usize, last: usize) -> Transaction {
    let mut seen: Vec<String> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    for line in (first..=last).map(|l| doc.line_content(l)) {
        if !seen.contains(&line) {
            seen.push(line.clone());
            kept.push(line);
        }
    }
    if kept.len() == last - first + 1 {
        return Transaction::new();
    }
    replace_block(doc, first, last, kept)
}

/// Удаление пустых строк (строки без символов; пробельные — не пустые).
pub fn remove_empty_lines(doc: &Document, first: usize, last: usize) -> Transaction {
    let kept: Vec<String> = (first..=last)
        .map(|l| doc.line_content(l))
        .filter(|l| !l.is_empty())
        .collect();
    if kept.len() == last - first + 1 {
        return Transaction::new();
    }
    replace_block(doc, first, last, kept)
}

/// Преобразование регистра в char-диапазонах (например, диапазоны
/// выделений; пустой срез — весь документ). 1:1 по длине для ASCII;
/// для символов с многозначным верхним регистром (ß → SS) длина может
/// измениться — осознанный компромисс (D9.6).
pub fn change_case(doc: &Document, ranges: &[(usize, usize)], mode: CaseMode) -> Transaction {
    let edits = ranges
        .iter()
        .filter(|(s, e)| e > s)
        .map(|(s, e)| {
            let text = doc.slice_str(*s, *e);
            Edit {
                start: *s,
                end: *e,
                text: match mode {
                    CaseMode::Upper => text.to_uppercase(),
                    CaseMode::Lower => text.to_lowercase(),
                    CaseMode::Title => title_case(&text),
                    CaseMode::Sentence => sentence_case(&text),
                },
            }
        })
        .collect();
    Transaction::from_edits(edits)
}

/// Заменить блок строк `[first..=last]` на готовые строки. Завершающий
/// `\n` добавляется, если последняя строка блока имеет терминатор
/// (блок остаётся отделённым от следующей строки / конца файла).
fn replace_block(doc: &Document, first: usize, last: usize, lines: Vec<String>) -> Transaction {
    let (start, end) = block_span(doc, first, last);
    let mut text = lines.join("\n");
    // Терминатор в конце блока: символ перед концом диапазона (корректно
    // и для фантомной хвостовой пустой строки ropey).
    if end > start && doc.slice_str(end - 1, end) == "\n" {
        text.push('\n');
    }
    if doc.slice_str(start, end) == text {
        return Transaction::new();
    }
    Transaction::from_edits(vec![Edit { start, end, text }])
}

/// Заголовочный регистр: первая буква каждого слова — верхняя, остальные
/// — нижняя. Слово начинается после любого не-буквенно-цифрового символа.
fn title_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at_word_start = true;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if at_word_start {
                out.extend(c.to_uppercase());
                at_word_start = false;
            } else {
                out.extend(c.to_lowercase());
            }
        } else {
            at_word_start = true;
            out.push(c);
        }
    }
    out
}

/// Регистр «как в предложении»: всё — нижний, первая буква после
/// границы предложения (`.`, `!`, `?` + пробелы, либо начало текста)
/// — верхняя.
fn sentence_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at_sentence_start = true;
    let mut after_punct = false;
    for c in text.chars() {
        if after_punct && c.is_whitespace() {
            at_sentence_start = true;
            after_punct = false;
            out.push(c);
            continue;
        }
        after_punct = matches!(c, '.' | '!' | '?');
        if at_sentence_start && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            at_sentence_start = false;
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(doc: &mut Document, tx: Transaction) {
        doc.apply(&tx);
    }

    /// Сценарий specs/text-editing «Сортировка выделения»: выделенные
    /// строки перестраиваются, остальной текст не меняется.
    #[test]
    fn sort_selection_scenario() {
        let mut doc = Document::from_str("header\npear\napple\nbanana\nfooter");
        // Выделение строк 1..=3.
        let tx = sort_lines(&doc, 1, 3, true, false);
        apply(&mut doc, tx);
        assert_eq!(doc.text(), "header\napple\nbanana\npear\nfooter");
        // По убыванию.
        let tx = sort_lines(&doc, 1, 3, false, false);
        apply(&mut doc, tx);
        assert_eq!(doc.text(), "header\npear\nbanana\napple\nfooter");
    }

    /// Сортировка без учёта регистра: латиница вперемешку с регистрами.
    #[test]
    fn sort_case_insensitive() {
        let mut doc = Document::from_str("Beta\nalpha\nCharlie\n");
        let tx = sort_lines(&doc, 0, 2, true, true);
        apply(&mut doc, tx);
        assert_eq!(doc.text(), "alpha\nBeta\nCharlie\n");
        // С учётом регистра (байтовый порядок): 'B' < 'a'.
        let mut doc = Document::from_str("beta\nAlpha\n");
        let tx = sort_lines(&doc, 0, 1, true, false);
        apply(&mut doc, tx);
        assert_eq!(doc.text(), "Alpha\nbeta\n");
    }

    /// Блок у конца файла без хвостового перевода строки не «склеивает»
    /// строки; внутренний блок сохраняет терминатор.
    #[test]
    fn sort_preserves_line_boundaries() {
        // EOF без \n.
        let mut doc = Document::from_str("b\na");
        let tx = sort_lines(&doc, 0, 1, true, false);
        apply(&mut doc, tx);
        assert_eq!(doc.text(), "a\nb");
        // Внутренний блок: терминатор последней строки остаётся.
        let mut doc = Document::from_str("x\nb\na\ny");
        let tx = sort_lines(&doc, 1, 2, true, false);
        apply(&mut doc, tx);
        assert_eq!(doc.text(), "x\na\nb\ny");
    }

    /// Сценарий «Удаление дубликатов»: по одному экземпляру, порядок
    /// первых вхождений.
    #[test]
    fn dedupe_scenario() {
        let mut doc = Document::from_str("b\na\nb\nc\na\nb\n");
        let tx = dedupe_lines(&doc, 0, 5);
        apply(&mut doc, tx);
        assert_eq!(doc.text(), "b\na\nc\n");
        // Нет дубликатов — no-op (пустая транзакция).
        let doc = Document::from_str("x\ny\n");
        assert!(dedupe_lines(&doc, 0, 1).is_empty());
    }

    /// Удаление пустых строк; пробельные строки остаются.
    #[test]
    fn remove_empty() {
        let mut doc = Document::from_str("a\n\nb\n   \nc\n");
        let tx = remove_empty_lines(&doc, 0, 4);
        apply(&mut doc, tx);
        assert_eq!(doc.text(), "a\nb\n   \nc\n");
    }

    /// Сценарий «Верхний регистр»: все буквы выделения заглавные,
    /// длина и остальной текст не меняются.
    #[test]
    fn upper_case_scenario() {
        let mut doc = Document::from_str("say hello, world! ok");
        let tx = change_case(&doc, &[(4, 17)], CaseMode::Upper);
        apply(&mut doc, tx);
        assert_eq!(doc.text(), "say HELLO, WORLD! ok");
        assert_eq!(doc.len_chars(), 20);
    }

    /// Нижний, заголовочный и sentence-регистры.
    #[test]
    fn lower_title_sentence() {
        let doc = Document::from_str("MiXeD CaSe TEXT");
        let mut d = Document::from_str("MiXeD CaSe TEXT");
        apply(&mut d, change_case(&doc, &[(0, 15)], CaseMode::Lower));
        assert_eq!(d.text(), "mixed case text");

        let mut d = Document::from_str("MiXeD CaSe TEXT");
        apply(&mut d, change_case(&doc, &[(0, 15)], CaseMode::Title));
        assert_eq!(d.text(), "Mixed Case Text");

        let doc2 = Document::from_str("hello. world! и yet: ANOTHER one");
        let mut d = Document::from_str("hello. world! и yet: ANOTHER one");
        apply(&mut d, change_case(&doc2, &[(0, doc2.len_chars())], CaseMode::Sentence));
        assert_eq!(d.text(), "Hello. World! И yet: another one");
    }

    /// Несколько диапазонов за одну транзакцию (мульти-выделение);
    /// пересекающиеся отбрасываются from_edits.
    #[test]
    fn case_on_multiple_ranges() {
        let mut doc = Document::from_str("aa bb cc dd");
        let tx = change_case(&doc, &[(0, 2), (6, 8)], CaseMode::Upper);
        apply(&mut doc, tx);
        assert_eq!(doc.text(), "AA bb CC dd");
    }

    /// Сортировка/дедуп одной строки — no-op.
    #[test]
    fn single_line_block_is_noop() {
        let doc = Document::from_str("only\n");
        assert!(sort_lines(&doc, 2, 2, true, false).is_empty());
        assert!(dedupe_lines(&doc, 0, 0).is_empty());
    }
}
