//! Документ — обёртка над ropey-`Rope`: streaming-загрузка из файла,
//! чтение строк, применение транзакций. Все координаты — char-индексы
//! (как в ropey 1.6).

use std::fs::File;
use std::io::{self, BufReader};
use std::path::Path;

use ropey::Rope;

use crate::encoding::FileFormat;
use crate::line_meta::LineMeta;
use crate::transaction::Transaction;

/// Текстовый документ.
#[derive(Debug)]
pub struct Document {
    rope: Rope,
    /// Метаданные строк (закладки, folded); синхронизируется с rope
    /// в `apply` — см. design.md D2.3.
    meta: LineMeta,
    /// Формат файла (кодировка + переводы строк); запоминается при
    /// загрузке и используется при сохранении — см. design.md D2.5.
    format: FileFormat,
}

impl Default for Document {
    fn default() -> Self {
        Document::new()
    }
}

impl Document {
    pub fn new() -> Self {
        Document { rope: Rope::new(), meta: LineMeta::with_lines(1), format: FileFormat::default() }
    }

    pub fn from_str(s: &str) -> Self {
        let rope = Rope::from_str(s);
        let lines = rope.len_lines();
        Document { rope, meta: LineMeta::with_lines(lines), format: FileFormat::default() }
    }

    pub fn from_rope(rope: Rope) -> Self {
        let lines = rope.len_lines();
        Document { rope, meta: LineMeta::with_lines(lines), format: FileFormat::default() }
    }

    /// Потоковая загрузка UTF-8 из файла: байты читаются в rope
    /// инкрементально через `Rope::from_reader` (файл целиком в память
    /// не попадает).
    pub fn from_reader(reader: impl io::Read) -> io::Result<Self> {
        let rope = Rope::from_reader(BufReader::new(reader))?;
        Ok(Document::from_rope(rope))
    }

    /// Потоковая загрузка UTF-8 файла по пути.
    pub fn load_file(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::from_reader(File::open(path)?)
    }

    /// Загрузка с определением кодировки и переводов строк (задача 2.5,
    /// D2.5): BOM → потоковая валидация UTF-8 → эвристика chardetng;
    /// декодирование lossy-streaming, модель LF-нормализована. Формат
    /// запоминается в документе для `save_file`.
    pub fn load_file_detected(path: impl AsRef<Path>) -> io::Result<Self> {
        let (rope, format) = crate::encoding::load_path(path.as_ref())?;
        let lines = rope.len_lines();
        Ok(Document { rope, meta: LineMeta::with_lines(lines), format })
    }

    /// Потоковое сохранение: обратная конвертация переводов строк
    /// в формат файла и кодирование в исходную кодировку (BOM
    /// переизлучается). Новые документы — UTF-8/LF.
    pub fn save_file(&self, path: impl AsRef<Path>) -> io::Result<()> {
        crate::encoding::save_path(&self.rope, path.as_ref(), &self.format)
    }

    /// Формат файла: кодировка + статистика переводов строк.
    pub fn format(&self) -> &FileFormat {
        &self.format
    }

    /// Кодировка файла (для статус-бара 3.5).
    pub fn encoding(&self) -> crate::encoding::Encoding {
        self.format.encoding
    }

    /// Статистика переводов строк исходного файла (для статус-бара 3.5).
    pub fn line_endings(&self) -> crate::encoding::LineEndingStats {
        self.format.line_endings
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn char_to_line(&self, char_idx: usize) -> usize {
        self.rope.char_to_line(char_idx)
    }

    pub fn line_to_char(&self, line_idx: usize) -> usize {
        self.rope.line_to_char(line_idx)
    }

    /// Сырая строка с терминатором (как в rope).
    pub fn line_str(&self, line_idx: usize) -> String {
        self.rope.line(line_idx).to_string()
    }

    /// Строка без терминатора (\n / \r\n).
    pub fn line_content(&self, line_idx: usize) -> String {
        let s = self.rope.line(line_idx).to_string();
        let s = s.strip_suffix('\n').unwrap_or(&s);
        s.strip_suffix('\r').unwrap_or(s).to_string()
    }

    /// Текст диапазона `[start, end)` в char-индексах.
    pub fn slice_str(&self, start: usize, end: usize) -> String {
        self.rope.slice(start..end).to_string()
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Метаданные строк: закладки, folded (задача 2.3).
    pub fn line_meta(&self) -> &LineMeta {
        &self.meta
    }

    /// Мутабельный доступ к метаданным строк (установка флагов).
    pub fn line_meta_mut(&mut self) -> &mut LineMeta {
        &mut self.meta
    }

    /// Применяет транзакцию, возвращает её инверсию (для undo, задача 2.4).
    /// Индекс метаданных строк синхронизируется до мутации rope — по
    /// до-транзакционному состоянию.
    pub fn apply(&mut self, tx: &Transaction) -> Transaction {
        self.meta.apply_transaction(tx, &self.rope);
        tx.apply(&mut self.rope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use crate::transaction::Edit;

    /// Задача 2.1: вставки и удаления в начале, середине, конце rope.
    #[test]
    fn insert_delete_at_start_middle_end() {
        let mut doc = Document::from_str("alpha beta gamma");
        // Начало.
        doc.apply(&Transaction::from_edits(vec![Edit::insert(0, ">> ")]));
        assert_eq!(doc.text(), ">> alpha beta gamma");
        // Середина.
        doc.apply(&Transaction::from_edits(vec![Edit::insert(11, "-")]));
        assert_eq!(doc.text(), ">> alpha be-ta gamma");
        // Конец.
        doc.apply(&Transaction::from_edits(vec![Edit::insert(doc.len_chars(), "!")]));
        assert_eq!(doc.text(), ">> alpha be-ta gamma!");
        // Удаления: начало, середина, конец.
        doc.apply(&Transaction::from_edits(vec![Edit::delete(0, 3)]));
        assert_eq!(doc.text(), "alpha be-ta gamma!");
        doc.apply(&Transaction::from_edits(vec![Edit::delete(8, 9)]));
        assert_eq!(doc.text(), "alpha beta gamma!");
        let len = doc.len_chars();
        doc.apply(&Transaction::from_edits(vec![Edit::delete(len - 1, len)]));
        assert_eq!(doc.text(), "alpha beta gamma");
    }

    /// Позиции правок внутри одной транзакции — до-транзакционные.
    #[test]
    fn multi_edit_positions_are_pre_transaction() {
        let mut doc = Document::from_str("abcdef");
        let tx = Transaction::from_edits(vec![
            Edit::insert(0, "X"),
            Edit::delete(2, 3),
            Edit::insert(6, "Y"),
        ]);
        doc.apply(&tx);
        assert_eq!(doc.text(), "XabdefY");
    }

    /// Транзакция возвращает инверсию, восстанавливающую документ.
    #[test]
    fn apply_returns_working_inverse() {
        let original = "hello world";
        let mut doc = Document::from_str(original);
        let tx = Transaction::from_edits(vec![
            Edit::insert(5, " beautiful"),
            Edit::delete(0, 1),
        ]);
        let inverse = doc.apply(&tx);
        assert_eq!(doc.text(), "ello beautiful world");
        doc.apply(&inverse);
        assert_eq!(doc.text(), original);
    }

    #[test]
    fn line_reads() {
        let doc = Document::from_str("one\ntwo\nthree\n");
        assert_eq!(doc.len_lines(), 4);
        assert_eq!(doc.line_content(0), "one");
        assert_eq!(doc.line_content(1), "two");
        assert_eq!(doc.line_content(2), "three");
        assert_eq!(doc.line_content(3), "");
        assert_eq!(doc.line_str(1), "two\n");
        assert_eq!(doc.line_to_char(2), 8);
        assert_eq!(doc.char_to_line(10), 2);
    }

    #[test]
    fn empty_document() {
        let mut doc = Document::new();
        assert_eq!(doc.len_lines(), 1);
        assert_eq!(doc.len_chars(), 0);
        doc.apply(&Transaction::from_edits(vec![Edit::insert(0, "x")]));
        assert_eq!(doc.text(), "x");
    }

    /// Задача 2.1: streaming-загрузка файла — rope строится из потока байт.
    #[test]
    fn from_reader_streams_utf8_file() {
        let dir = TempDir::new("stream");
        let path = dir.path("plain.txt");
        std::fs::write(&path, "line one\nline two\nline three\n").unwrap();

        let doc = Document::load_file(&path).unwrap();
        assert_eq!(doc.text(), "line one\nline two\nline three\n");
        assert_eq!(doc.len_lines(), 4);
        assert_eq!(doc.line_content(1), "line two");
    }
}
