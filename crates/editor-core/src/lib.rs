//! editor-core — текстовая модель редактора без зависимостей от GUI.
//!
//! Состав (группа 2): ropey-документ со streaming-загрузкой и индексом
//! метаданных строк (document, line_meta), атомарные правки и транзакции
//! (transaction), выделения и multi-cursor транзакции (selection),
//! undo/redo с группировкой ввода (undo), кодировки и переводы строк
//! (encoding).

pub mod document;
pub mod encoding;
pub mod line_meta;
pub mod line_ops;
pub mod selection;
pub mod transaction;
pub mod undo;

#[cfg(test)]
mod test_support;

pub use document::Document;
pub use encoding::{Encoding, FileFormat, LineEnding, LineEndingStats};
pub use line_meta::LineMeta;
pub use line_ops::{change_case, dedupe_lines, remove_empty_lines, sort_lines, CaseMode};
pub use selection::{Selection, backspace, forward_delete, insert_text};
pub use transaction::{Bias, Edit, Transaction};
pub use undo::History;
