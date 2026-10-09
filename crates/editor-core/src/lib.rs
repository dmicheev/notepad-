//! editor-core — текстовая модель редактора без зависимостей от GUI.
//!
//! Состав (группа 2): ropey-документ со streaming-загрузкой и индексом
//! метаданных строк (document, line_meta), атомарные правки и транзакции
//! (transaction), выделения и multi-cursor транзакции (selection),
//! undo/redo с группировкой ввода (undo), кодировки и переводы строк
//! (encoding). Группа 5: определение языка (syntax) и движок подсветки
//! видимых строк (highlight) на syntect. Группа 6: поиск и замена —
//! движок по документу и скан папки (search).

pub mod document;
pub mod encoding;
pub mod highlight;
pub mod json;
pub mod line_meta;
pub mod line_ops;
pub mod search;
pub mod selection;
pub mod syntax;
pub mod transaction;
pub mod undo;

#[cfg(test)]
mod test_support;

pub use syntect::parsing::SyntaxReference;

/// Реэкспорт ropey: app-слой работает с `Document::rope()` (фоновый
/// поиск по вкладкам клонирует ropes) — без новой зависимости app.
pub use ropey;

pub use document::Document;
pub use encoding::{Encoding, FileFormat, LineEnding, LineEndingStats};
pub use highlight::{CsvOptions, HighlightEngine, Span, StyleClass};
pub use json::{JsonError, JsonStyle, format_json};
pub use line_meta::LineMeta;
pub use line_ops::{change_case, dedupe_lines, remove_empty_lines, sort_lines, CaseMode};
pub use search::{
    FileHit, FileScan, ScanSummary, SearchMatcher, SearchOptions, collect_files, glob_match,
    scan_file, scan_paths,
};
pub use selection::{Selection, backspace, forward_delete, insert_text};
pub use syntax::{
    detect, detect_by_extension, detect_by_first_line, detect_labeled, friendly_name, plain_text,
    syntax_by_name, syntax_set, try_syntax_set,
};
pub use transaction::{Bias, Edit, Transaction};
pub use undo::History;
