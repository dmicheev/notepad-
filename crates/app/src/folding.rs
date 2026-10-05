//! Свёртывание блоков по отступам (задача 4.5, design.md D9.5).
//!
//! MVP-подход: блок = строка-заголовок, за которой следует непрерывный
//! диапазон строк с БОЛЬШИМ отступом; пустые строки принадлежат телу,
//! если за ними в блоке есть непустые. Синтаксисные диапазоны (tree-
//! sitter/syntect) заменят эту логику в 5.1 — интерфейс функций не
//! изменится. Folded-флаг хранится в `LineMeta` заголовка (переживает
//! undo/redo — синхронизация внутри `Document::apply`).
//!
//! uniform_list индексирует строки напрямую, поэтому виджет рендерит
//! СПИСОК ВИДИМЫХ строк: `visible_lines` строит мапу visible→real,
//! обратный переход — бинарный поиск. Мапа пересчитывается при правках
//! и переключении флагов (версия вкладки — `BufferTab::view_version`).

use editor_core::Document;

/// Отступ строки: пробел = 1, таб = 4 (единая семантика кликов/фолдов).
pub fn indent_of(line: &str) -> usize {
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

/// Последняя строка тела блока с заголовком `header` (не пустая).
/// Пустые строки в конце блока не продлевают тело.
pub fn fold_end(doc: &Document, header: usize) -> usize {
    let level = indent_of(&doc.line_content(header));
    let mut last = header;
    for line in header + 1..doc.len_lines() {
        let content = doc.line_content(line);
        if content.trim().is_empty() {
            continue; // пустая — кандидат в тело
        }
        if indent_of(&content) > level {
            last = line;
        } else {
            break;
        }
    }
    last
}

/// Строка — заголовок сворачиваемого блока (есть непустое тело).
pub fn is_foldable(doc: &Document, line: usize) -> bool {
    fold_end(doc, line) > line
}

/// Мапа видимых строк (visible→real): строки тел свёрнутых блоков
/// пропускаются. Пустой документ даёт единственную строку 0.
pub fn visible_lines(doc: &Document) -> Vec<usize> {
    let total = doc.len_lines();
    let meta = doc.line_meta();
    let mut out = Vec::with_capacity(total);
    let mut line = 0;
    while line < total {
        out.push(line);
        if meta.is_folded(line) && is_foldable(doc, line) {
            line = fold_end(doc, line);
        }
        line += 1;
    }
    out
}

/// real-строка → индекс в мапе видимых; `None` — строка скрыта.
pub fn real_to_visible(map: &[usize], line: usize) -> Option<usize> {
    map.binary_search(&line).ok()
}

/// Заголовок свёрнутого блока, в теле которого находится `line`
/// (или `None` — строка видима). Заголовки сканируются по возрастанию.
pub fn enclosing_fold_header(doc: &Document, line: usize) -> Option<usize> {
    let meta = doc.line_meta();
    for header in meta.folded_lines() {
        if header >= line {
            break;
        }
        if is_foldable(doc, header) && fold_end(doc, header) >= line {
            return Some(header);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "\
fn main() {
    if cond {
        deep();
    }
    tail();
}
top();
";

    #[test]
    fn indent_of_mixed() {
        assert_eq!(indent_of("no indent"), 0);
        assert_eq!(indent_of("  two spaces"), 2);
        assert_eq!(indent_of("\t\ttab"), 8);
        assert_eq!(indent_of("\t space"), 5);
        assert_eq!(indent_of(""), 0);
    }

    #[test]
    fn fold_end_and_foldable() {
        let doc = Document::from_text(CODE);
        // 0: fn main; 1: if cond; 2: deep; 3: tail?; нет —
        // 0 "fn main() {", 1 "  if cond {", 2 "    deep();",
        // 3 "  }", 4 "  tail();", 5 "}", 6 "top();", 7 "".
        assert!(is_foldable(&doc, 0));
        assert_eq!(fold_end(&doc, 0), 4); // тело до "  tail();" включительно
        assert!(is_foldable(&doc, 1));
        assert_eq!(fold_end(&doc, 1), 2); // только "deep();"
        assert!(!is_foldable(&doc, 2));
        assert!(!is_foldable(&doc, 4));
        assert!(!is_foldable(&doc, 6));
    }

    #[test]
    fn blank_lines_belong_to_body() {
        // Пустая строка внутри блока не рвёт тело; хвостовые пустые — не тело.
        let doc = Document::from_text("head:\n    a\n\n    b\n\ntail\n");
        assert_eq!(fold_end(&doc, 0), 3); // до "    b", пустая 4 не в теле
        assert!(is_foldable(&doc, 0));
    }

    #[test]
    fn visible_mapping_skips_folded_bodies() {
        let mut doc = Document::from_text(CODE);
        // Всё видно — тождественная мапа.
        assert_eq!(visible_lines(&doc), (0..8).collect::<Vec<_>>());
        // Свернули fn main (строка 0, тело 1..=4).
        doc.line_meta_mut().set_folded(0, true);
        assert_eq!(visible_lines(&doc), vec![0, 5, 6, 7]);
        // Свернули и вложенный if — всё равно скрыт телом внешнего.
        doc.line_meta_mut().set_folded(1, true);
        assert_eq!(visible_lines(&doc), vec![0, 5, 6, 7]);
        // Развернули внешний, свернули внутренний: скрыта строка 2.
        doc.line_meta_mut().set_folded(0, false);
        assert_eq!(visible_lines(&doc), vec![0, 1, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn real_visible_conversions() {
        let mut doc = Document::from_text(CODE);
        doc.line_meta_mut().set_folded(0, true);
        let map = visible_lines(&doc); // [0, 5, 6, 7]
        assert_eq!(real_to_visible(&map, 5), Some(1));
        assert_eq!(real_to_visible(&map, 0), Some(0));
        assert_eq!(real_to_visible(&map, 2), None, "скрытая строка");
        assert_eq!(enclosing_fold_header(&doc, 2), Some(0));
        assert_eq!(enclosing_fold_header(&doc, 0), None);
        assert_eq!(enclosing_fold_header(&doc, 6), None);
    }
}
