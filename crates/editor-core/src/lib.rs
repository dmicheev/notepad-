//! editor-core — текстовая модель редактора без зависимостей от GUI.
//!
//! Состав (группа 2): ropey-документ со streaming-загрузкой и индексом
//! метаданных строк (document, line_meta), атомарные правки и транзакции
//! (transaction), выделения и multi-cursor транзакции (selection),
//! undo/redo с группировкой ввода (undo), кодировки и переводы строк
//! (encoding). Группа 5: определение языка (syntax) и движок подсветки
//! видимых строк (highlight) на syntect.

pub mod document;
pub mod encoding;
pub mod highlight;
pub mod line_meta;
pub mod line_ops;
pub mod selection;
pub mod syntax;
pub mod transaction;
pub mod undo;

#[cfg(test)]
mod test_support;

pub use syntect::parsing::SyntaxReference;

pub use document::Document;
pub use encoding::{Encoding, FileFormat, LineEnding, LineEndingStats};
pub use highlight::{HighlightEngine, Span, StyleClass};
pub use line_meta::LineMeta;
pub use line_ops::{change_case, dedupe_lines, remove_empty_lines, sort_lines, CaseMode};
pub use selection::{Selection, backspace, forward_delete, insert_text};
pub use syntax::{
    detect, detect_by_extension, detect_by_first_line, detect_labeled, friendly_name, plain_text,
    syntax_by_name, syntax_set, try_syntax_set,
};
pub use transaction::{Bias, Edit, Transaction};
pub use undo::History;
