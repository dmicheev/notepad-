//! Кодировки и переводы строк (задача 2.5, решения D2.5/D6).
//!
//! Загрузка — потоковая, файл целиком в память не попадает: читается
//! кусками по 64 КБ, декодированный текст инкрементально строится в
//! rope через `ropey::RopeBuilder` (в памяти только префикс-кусок).
//! Определение кодировки: BOM (UTF-8 / UTF-16LE / UTF-16BE — декодирование
//! фиксированное) → потоковая валидация UTF-8 с одновременным кормлением
//! chardetng → при первом же malformed байте эвристика `guess`
//! и перечитывание файла с начала в кодировке encoding_rs. Декодирование
//! lossy (невалидные последовательности → U+FFFD): файл невозможно
//! «не открыть»; файлы без BOM, прошедшие UTF-8-валидацию, — без потерь.
//!
//! Переводы строк: при загрузке модель нормализуется в LF (D6),
//! статистика LF/CRLF/CR считается по исходному тексту; при сохранении —
//! обратная конвертация в доминирующий формат файла. Round-trip
//! байт-в-байты гарантирован для файлов с единообразными переводами
//! строк; смешанные нормализуются в доминирующий формат. BOM
//! сохраняется. Новые документы: UTF-8 без BOM, LF.

use std::fs::File;
use std::io::{self, BufWriter, Cursor, Read, Write};
use std::path::Path;

use encoding_rs::{CoderResult, DecoderResult, Encoding as RsEncoding};
use ropey::{Rope, RopeBuilder};

/// Размер куска потокового чтения.
const CHUNK: usize = 64 * 1024;

/// Формат перевода строки.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    /// Подпись для статус-бара (3.5).
    pub fn label(&self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
            LineEnding::Cr => "CR",
        }
    }
}

/// Статистика переводов строк исходного файла (до нормализации).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineEndingStats {
    pub lf: usize,
    pub crlf: usize,
    pub cr: usize,
}

impl LineEndingStats {
    /// Доминирующий формат файла: большинство переводов строк;
    /// при равенстве приоритет LF &gt; CRLF &gt; CR; отсутствие
    /// переводов — LF (дефолт).
    pub fn dominant(&self) -> LineEnding {
        if self.lf >= self.crlf && self.lf >= self.cr {
            LineEnding::Lf
        } else if self.crlf >= self.cr {
            LineEnding::CrLf
        } else {
            LineEnding::Cr
        }
    }
}

/// Кодировка файла: кодировка encoding_rs + признак BOM
/// (BOM переизлучается при сохранении — round-trip байт-в-байты).
#[derive(Clone, Copy)]
pub struct Encoding {
    enc: &'static RsEncoding,
    bom: bool,
}

impl PartialEq for Encoding {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.enc, other.enc) && self.bom == other.bom
    }
}

impl Eq for Encoding {}

impl std::fmt::Debug for Encoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoding").field("name", &self.enc.name()).field("bom", &self.bom).finish()
    }
}

impl Encoding {
    pub const fn utf8() -> Self {
        Encoding { enc: encoding_rs::UTF_8, bom: false }
    }

    pub const fn utf8_bom() -> Self {
        Encoding { enc: encoding_rs::UTF_8, bom: true }
    }

    /// Файл с BOM в кодировке `enc` (UTF-8 / UTF-16LE / UTF-16BE).
    pub const fn with_bom(enc: &'static RsEncoding) -> Self {
        Encoding { enc, bom: true }
    }

    /// Файл без BOM (например, windows-1251 по эвристике chardetng).
    pub const fn without_bom(enc: &'static RsEncoding) -> Self {
        Encoding { enc, bom: false }
    }

    /// Кодировка encoding_rs для декодирования/кодирования.
    pub fn rs(&self) -> &'static RsEncoding {
        self.enc
    }

    /// Была ли BOM-подпись в файле (переизлучается при сохранении).
    pub fn has_bom(&self) -> bool {
        self.bom
    }

    /// Подпись для статус-бара (3.5).
    pub fn label(&self) -> String {
        if self.bom && std::ptr::eq(self.enc, encoding_rs::UTF_8) {
            "UTF-8 (BOM)".to_string()
        } else {
            self.enc.name().to_string()
        }
    }
}

/// Формат файла: кодировка + статистика переводов строк. Запоминается
/// при загрузке (`Document::load_file_detected`) и используется
/// при сохранении (`Document::save_file`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileFormat {
    pub encoding: Encoding,
    pub line_endings: LineEndingStats,
}

impl Default for FileFormat {
    /// Дефолт для новых документов: UTF-8 без BOM, LF.
    fn default() -> Self {
        FileFormat { encoding: Encoding::utf8(), line_endings: LineEndingStats::default() }
    }
}

/// Загружает файл: определение кодировки (BOM → UTF-8 валидация →
/// chardetng), декодирование и нормализация переводов строк — потоково.
/// Возвращает rope (LF-нормализованный) и определённый формат.
pub(crate) fn load_path(path: &Path) -> io::Result<(Rope, FileFormat)> {
    let mut file = File::open(path)?;
    let mut prefix = vec![0u8; CHUNK];
    let n = fill(&mut file, &mut prefix)?;
    prefix.truncate(n);
    if prefix.is_empty() {
        return Ok((Rope::new(), FileFormat::default()));
    }

    // 1. BOM — кодировка фиксирована, декодируем lossy-streaming.
    if let Some((enc, bom_len)) = bom_encoding(&prefix) {
        let mut reader = Cursor::new(&prefix[bom_len..]).chain(&mut file);
        let (rope, line_endings) = decode_stream(&mut reader, enc)?;
        return Ok((rope, FileFormat { encoding: Encoding::with_bom(enc), line_endings }));
    }

    // 2. Потоковая валидация UTF-8; rope строится на лету, те же куски
    //    параллельно кормятся детектору chardetng на случай fallback.
    let mut detector = chardetng::EncodingDetector::new();
    let mut decoder = encoding_rs::UTF_8.new_decoder_without_bom_handling();
    let mut builder = RopeBuilder::new();
    let mut norm = NewlineNormalizer::new();
    let mut buf = vec![0u8; CHUNK];
    let mut out = String::with_capacity(CHUNK * 3 + 16);
    let mut valid = true;
    let mut first = true;
    loop {
        let bytes: &[u8] = if first {
            first = false;
            &prefix
        } else {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            &buf[..n]
        };
        detector.feed(bytes, false);
        out.clear();
        let (res, _) = decoder.decode_to_string_without_replacement(bytes, &mut out, false);
        if matches!(res, DecoderResult::Malformed(..)) {
            valid = false;
            break;
        }
        builder.append(norm.push(&out));
    }
    if valid {
        // Финальный flush: незавершённая много-байтовая последовательность
        // в конце файла тоже делает UTF-8 невалидным.
        out.clear();
        let (res, _) = decoder.decode_to_string_without_replacement(&[], &mut out, true);
        if !matches!(res, DecoderResult::Malformed(..)) {
            if !out.is_empty() {
                builder.append(norm.push(&out));
            }
            builder.append(norm.finish());
            return Ok((
                builder.finish(),
                FileFormat { encoding: Encoding::utf8(), line_endings: norm.stats() },
            ));
        }
    }

    // 3. UTF-8 не прошёл валидацию — эвристика chardetng по накопленным
    //    кускам; файл перечитывается с начала в предположенной кодировке.
    let enc = detector.guess(None, false);
    let mut file2 = File::open(path)?;
    let (rope, line_endings) = decode_stream(&mut file2, enc)?;
    Ok((rope, FileFormat { encoding: Encoding::without_bom(enc), line_endings }))
}

/// Потоковое сохранение: обратная конвертация переводов строк в формат
/// файла и кодирование в исходную кодировку (BOM переизлучается).
/// Rope обходится чанками, без сборки всего текста в строку.
/// UTF-16 — своей конвертацией: encoding_rs не даёт encoder для UTF-16
/// (output encoding UTF-16 не поддерживается, `new_encoder()` вернул бы
/// UTF-8), а декодирование UTF-16 через encoding_rs работает.
pub(crate) fn save_path(rope: &Rope, path: &Path, fmt: &FileFormat) -> io::Result<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    if fmt.encoding.bom {
        writer.write_all(bom_bytes(fmt.encoding.enc))?;
    }
    let ending = fmt.line_endings.dominant();
    let utf16le = std::ptr::eq(fmt.encoding.enc, encoding_rs::UTF_16LE);
    let utf16be = std::ptr::eq(fmt.encoding.enc, encoding_rs::UTF_16BE);
    let mut encoder = fmt.encoding.enc.new_encoder();
    let mut text = String::new();
    let mut bytes = Vec::new();
    let mut dst = vec![0u8; 16 * 1024];
    let mut chunks = rope.chunks().peekable();
    while let Some(chunk) = chunks.next() {
        text.clear();
        push_converted(chunk, ending, &mut text);
        if utf16le || utf16be {
            bytes.clear();
            for unit in text.encode_utf16() {
                if utf16le {
                    bytes.extend_from_slice(&unit.to_le_bytes());
                } else {
                    bytes.extend_from_slice(&unit.to_be_bytes());
                }
            }
            writer.write_all(&bytes)?;
        } else {
            let last = chunks.peek().is_none();
            let mut consumed = 0;
            loop {
                let (res, read, written, _) =
                    encoder.encode_from_utf8(&text[consumed..], &mut dst, last);
                writer.write_all(&dst[..written])?;
                consumed += read;
                match res {
                    CoderResult::InputEmpty => break,
                    // Выходной буфер переполнен — кодируем остаток куска.
                    CoderResult::OutputFull => continue,
                }
            }
        }
    }
    writer.flush()
}

/// Декодирует поток в кодировке `enc` (lossy, U+FFFD на невалидных
/// последовательностях), нормализует переводы строк и инкрементально
/// строит rope. Возвращает rope и статистику переводов строк.
fn decode_stream(reader: &mut dyn Read, enc: &'static RsEncoding) -> io::Result<(Rope, LineEndingStats)> {
    let mut decoder = enc.new_decoder_without_bom_handling();
    let mut builder = RopeBuilder::new();
    let mut norm = NewlineNormalizer::new();
    let mut buf = vec![0u8; CHUNK];
    let mut out = String::with_capacity(CHUNK * 3 + 16);
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.clear();
        let _ = decoder.decode_to_string(&buf[..n], &mut out, false);
        builder.append(norm.push(&out));
    }
    // Финальный flush декодера (pending-байты незавершённой последовательности).
    out.clear();
    let _ = decoder.decode_to_string(&[], &mut out, true);
    if !out.is_empty() {
        builder.append(norm.push(&out));
    }
    builder.append(norm.finish());
    Ok((builder.finish(), norm.stats()))
}

/// Читает в `buf` до заполнения или EOF; возвращает число прочитанных байт.
fn fill<R: Read>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        let n = reader.read(&mut buf[total..])?;
        if n == 0 {
            break;
        }
        total += n;
    }
    Ok(total)
}

/// BOM-подпись в начале префикса: (кодировка, длина BOM в байтах).
fn bom_encoding(prefix: &[u8]) -> Option<(&'static RsEncoding, usize)> {
    if prefix.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some((encoding_rs::UTF_8, 3))
    } else if prefix.starts_with(&[0xFF, 0xFE]) {
        Some((encoding_rs::UTF_16LE, 2))
    } else if prefix.starts_with(&[0xFE, 0xFF]) {
        Some((encoding_rs::UTF_16BE, 2))
    } else {
        None
    }
}

/// Байты BOM для кодировки (пустые, если кодировка без BOM).
fn bom_bytes(enc: &'static RsEncoding) -> &'static [u8] {
    if std::ptr::eq(enc, encoding_rs::UTF_8) {
        &[0xEF, 0xBB, 0xBF]
    } else if std::ptr::eq(enc, encoding_rs::UTF_16LE) {
        &[0xFF, 0xFE]
    } else if std::ptr::eq(enc, encoding_rs::UTF_16BE) {
        &[0xFE, 0xFF]
    } else {
        &[]
    }
}

/// Конвертирует переводы строк куска из модели (LF) в целевой формат.
fn push_converted(chunk: &str, ending: LineEnding, out: &mut String) {
    match ending {
        LineEnding::Lf => out.push_str(chunk),
        LineEnding::CrLf => {
            for c in chunk.chars() {
                if c == '\n' {
                    out.push_str("\r\n");
                } else {
                    out.push(c);
                }
            }
        }
        LineEnding::Cr => {
            for c in chunk.chars() {
                if c == '\n' {
                    out.push('\r');
                } else {
                    out.push(c);
                }
            }
        }
    }
}

/// Конечный автомат нормализации переводов строк: CRLF и одинокие CR → LF
/// (модель LF-нормализована, D6), перенос `\r|\n` через границу кусков —
/// через флаг pending_cr. Заодно считает статистику по исходному тексту.
#[derive(Debug)]
struct NewlineNormalizer {
    pending_cr: bool,
    stats: LineEndingStats,
    out: String,
}

impl NewlineNormalizer {
    fn new() -> Self {
        NewlineNormalizer {
            pending_cr: false,
            stats: LineEndingStats::default(),
            out: String::with_capacity(CHUNK * 3 + 16),
        }
    }

    /// Проглатывает кусок декодированного текста, возвращает нормализованный.
    fn push(&mut self, chunk: &str) -> &str {
        self.out.clear();
        for c in chunk.chars() {
            if self.pending_cr {
                self.pending_cr = false;
                if c == '\n' {
                    self.stats.crlf += 1;
                    self.out.push('\n');
                } else {
                    // Одинокий CR закрывается, текущий символ — обычным путём.
                    self.stats.cr += 1;
                    self.out.push('\n');
                    self.handle(c);
                }
            } else {
                self.handle(c);
            }
        }
        &self.out
    }

    /// Завершает поток; одинокий CR в самом конце файла тоже становится LF.
    fn finish(&mut self) -> &str {
        self.out.clear();
        if self.pending_cr {
            self.pending_cr = false;
            self.stats.cr += 1;
            self.out.push('\n');
        }
        &self.out
    }

    fn stats(&self) -> LineEndingStats {
        self.stats
    }

    fn handle(&mut self, c: char) {
        if c == '\r' {
            self.pending_cr = true;
        } else {
            if c == '\n' {
                self.stats.lf += 1;
            }
            self.out.push(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::test_support::TempDir;
    use encoding_rs::{UTF_16BE, UTF_16LE, UTF_8, WINDOWS_1251};

    const TEXT_FULL: &str = "Первая строка 🙂\nВторая строка с ё и Й\nКонец файла\n";
    const TEXT_CYR: &str = "Записная книжка\nПривет, мир! Ёлка и йога\nПогода замечательная\n";

    fn with_endings(model: &str, ending: LineEnding) -> String {
        match ending {
            LineEnding::Lf => model.to_string(),
            LineEnding::CrLf => model.replace('\n', "\r\n"),
            LineEnding::Cr => model.replace('\n', "\r"),
        }
    }

    /// Байты файла: BOM + текст в заданной кодировке. UTF-16 — вручную
    /// (у encoding_rs UTF-16 — не output encoding).
    fn encode_file(enc: &'static RsEncoding, bom: bool, text: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(text.len() + 3);
        if bom {
            out.extend_from_slice(bom_bytes(enc));
        }
        if std::ptr::eq(enc, encoding_rs::UTF_16LE) {
            for unit in text.encode_utf16() {
                out.extend_from_slice(&unit.to_le_bytes());
            }
        } else if std::ptr::eq(enc, encoding_rs::UTF_16BE) {
            for unit in text.encode_utf16() {
                out.extend_from_slice(&unit.to_be_bytes());
            }
        } else {
            let (bytes, _, _) = enc.encode(text);
            out.extend_from_slice(&bytes);
        }
        out
    }

    /// Load → save → байты идентичны (общая проверка round-trip).
    fn assert_round_trip(dir: &TempDir, name: &str, original: &[u8]) -> Document {
        let src = dir.path(&format!("{name}.in"));
        let dst = dir.path(&format!("{name}.out"));
        std::fs::write(&src, original).unwrap();
        let doc = Document::load_file_detected(&src).unwrap();
        doc.save_file(&dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), original, "round-trip {name}");
        doc
    }

    /// Обязательный тест 2.5: round-trip для каждой кодировки × каждого
    /// формата переводов строк.
    #[test]
    fn round_trip_all_encodings_and_line_endings() {
        let dir = TempDir::new("matrix");
        let cases: &[(&'static RsEncoding, bool, &str)] = &[
            (UTF_8, false, TEXT_FULL),
            (UTF_8, true, TEXT_FULL),
            (UTF_16LE, true, TEXT_FULL),
            (UTF_16BE, true, TEXT_FULL),
            (WINDOWS_1251, false, TEXT_CYR),
        ];
        for (enc, bom, text) in cases {
            for ending in [LineEnding::Lf, LineEnding::CrLf, LineEnding::Cr] {
                let bytes = encode_file(enc, *bom, &with_endings(text, ending));
                let name = format!("{}-{}-{}", enc.name(), bom, ending.label());
                let doc = assert_round_trip(&dir, &name, &bytes);

                // Модель нормализована в LF.
                assert_eq!(doc.text(), *text, "текст {name}");
                // Статистика по исходному файлу: 3 перевода одного формата.
                let (lf, crlf, cr) = match ending {
                    LineEnding::Lf => (3, 0, 0),
                    LineEnding::CrLf => (0, 3, 0),
                    LineEnding::Cr => (0, 0, 3),
                };
                assert_eq!(
                    doc.line_endings(),
                    LineEndingStats { lf, crlf, cr },
                    "статистика {name}"
                );
                assert_eq!(doc.line_endings().dominant(), ending, "доминанта {name}");
                // Кодировка определена верно.
                let expected = if *bom { Encoding::with_bom(enc) } else { Encoding::without_bom(enc) };
                assert_eq!(doc.encoding(), expected, "кодировка {name}");
            }
        }
    }

    #[test]
    fn utf8_without_bom() {
        let dir = TempDir::new("utf8");
        let doc = assert_round_trip(&dir, "utf8", TEXT_FULL.as_bytes());
        assert_eq!(doc.encoding(), Encoding::utf8());
        assert_eq!(doc.encoding().label(), "UTF-8");
        assert_eq!(doc.line_endings(), LineEndingStats { lf: 3, crlf: 0, cr: 0 });
    }

    /// BOM срезается из текста модели и переизлучается при сохранении.
    #[test]
    fn utf8_bom_stripped_from_text_and_preserved_on_save() {
        let dir = TempDir::new("utf8bom");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(TEXT_FULL.as_bytes());
        let doc = assert_round_trip(&dir, "utf8bom", &bytes);
        assert_eq!(doc.encoding(), Encoding::utf8_bom());
        assert_eq!(doc.encoding().label(), "UTF-8 (BOM)");
        assert!(!doc.text().starts_with('\u{FEFF}'));
        assert_eq!(doc.text(), TEXT_FULL);
    }

    #[test]
    fn utf16_le_with_bom() {
        let dir = TempDir::new("utf16le");
        let bytes = encode_file(UTF_16LE, true, TEXT_FULL);
        let doc = assert_round_trip(&dir, "utf16le", &bytes);
        assert_eq!(doc.encoding(), Encoding::with_bom(UTF_16LE));
        assert_eq!(doc.encoding().label(), "UTF-16LE");
        assert_eq!(doc.text(), TEXT_FULL);
    }

    #[test]
    fn utf16_be_with_bom() {
        let dir = TempDir::new("utf16be");
        let bytes = encode_file(UTF_16BE, true, TEXT_FULL);
        let doc = assert_round_trip(&dir, "utf16be", &bytes);
        assert_eq!(doc.encoding(), Encoding::with_bom(UTF_16BE));
        assert_eq!(doc.text(), TEXT_FULL);
    }

    /// Windows-1251: файл невалиден как UTF-8 → fallback на chardetng;
    /// эвристика должна определить кодировку и декодировать текст.
    #[test]
    fn windows_1251_detected_via_chardetng() {
        let dir = TempDir::new("cp1251");
        let bytes = encode_file(WINDOWS_1251, false, TEXT_CYR);
        let doc = assert_round_trip(&dir, "cp1251", &bytes);
        assert_eq!(doc.encoding(), Encoding::without_bom(WINDOWS_1251));
        assert_eq!(doc.encoding().label(), "windows-1251");
        assert_eq!(doc.text(), TEXT_CYR);
    }

    #[test]
    fn crlf_normalized_on_load_restored_on_save() {
        let dir = TempDir::new("crlf");
        let doc = assert_round_trip(&dir, "crlf", b"one\r\ntwo\r\nthree\r\n");
        assert_eq!(doc.text(), "one\ntwo\nthree\n");
        assert_eq!(doc.line_endings(), LineEndingStats { lf: 0, crlf: 3, cr: 0 });
        assert_eq!(doc.line_endings().dominant(), LineEnding::CrLf);
    }

    #[test]
    fn lone_cr_normalized_on_load_restored_on_save() {
        let dir = TempDir::new("cr");
        let doc = assert_round_trip(&dir, "cr", b"a\rb\rc\r");
        assert_eq!(doc.text(), "a\nb\nc\n");
        assert_eq!(doc.line_endings(), LineEndingStats { lf: 0, crlf: 0, cr: 3 });
        assert_eq!(doc.line_endings().dominant(), LineEnding::Cr);
    }

    /// Смешанные переводы: доминирующий формат побеждает, сохранение
    /// нормализует всё в него (round-trip байт сознательно не сохраняется).
    #[test]
    fn mixed_endings_saved_in_dominant_format() {
        let dir = TempDir::new("mixed");
        // Большинство CRLF.
        let src = dir.path("majority.in");
        std::fs::write(&src, b"a\r\nb\r\nc\r\nd\n").unwrap();
        let doc = Document::load_file_detected(&src).unwrap();
        assert_eq!(doc.text(), "a\nb\nc\nd\n");
        assert_eq!(doc.line_endings(), LineEndingStats { lf: 1, crlf: 3, cr: 0 });
        assert_eq!(doc.line_endings().dominant(), LineEnding::CrLf);
        let dst = dir.path("majority.out");
        doc.save_file(&dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"a\r\nb\r\nc\r\nd\r\n");

        // Полная ничья 1/1/1 — приоритет LF.
        let src2 = dir.path("tie.in");
        std::fs::write(&src2, b"a\r\nb\nc\r").unwrap();
        let doc2 = Document::load_file_detected(&src2).unwrap();
        assert_eq!(doc2.line_endings(), LineEndingStats { lf: 1, crlf: 1, cr: 1 });
        assert_eq!(doc2.line_endings().dominant(), LineEnding::Lf);
        let dst2 = dir.path("tie.out");
        doc2.save_file(&dst2).unwrap();
        assert_eq!(std::fs::read(&dst2).unwrap(), b"a\nb\nc\n");
    }

    #[test]
    fn file_without_line_endings() {
        let dir = TempDir::new("noendings");
        let doc = assert_round_trip(&dir, "plain", b"hello");
        assert_eq!(doc.line_endings(), LineEndingStats::default());
        assert_eq!(doc.line_endings().dominant(), LineEnding::Lf);
    }

    #[test]
    fn empty_file() {
        let dir = TempDir::new("empty");
        let doc = assert_round_trip(&dir, "empty", b"");
        assert_eq!(doc.text(), "");
        assert_eq!(doc.encoding(), Encoding::utf8());
        assert_eq!(doc.len_lines(), 1);
    }

    /// Файл из одной только BOM: пустая модель, BOM сохраняется.
    #[test]
    fn bom_only_files() {
        let dir = TempDir::new("bomonly");
        let doc = assert_round_trip(&dir, "utf8", &[0xEF, 0xBB, 0xBF]);
        assert_eq!(doc.text(), "");
        assert_eq!(doc.encoding(), Encoding::utf8_bom());

        let doc = assert_round_trip(&dir, "utf16le", &[0xFF, 0xFE]);
        assert_eq!(doc.text(), "");
        assert_eq!(doc.encoding(), Encoding::with_bom(UTF_16LE));
    }

    /// Большой UTF-16LE файл: несколько кусков потокового чтения,
    /// статистика и round-trip без искажений.
    #[test]
    fn large_utf16_file_streams_across_chunks() {
        let dir = TempDir::new("large");
        let text = "Строка с emoji 🙂 и кириллицей\n".repeat(6000);
        assert!(text.len() > 300_000, "тест должен покрывать несколько кусков по 64 КБ");
        let bytes = encode_file(UTF_16LE, true, &text);
        assert!(bytes.len() > 350_000);

        let doc = assert_round_trip(&dir, "large", &bytes);
        assert_eq!(doc.len_chars(), text.chars().count());
        assert_eq!(doc.text(), text);
        assert_eq!(doc.line_endings(), LineEndingStats { lf: 6000, crlf: 0, cr: 0 });
    }

    /// Новый документ сохраняется как UTF-8/LF (дефолт формата).
    #[test]
    fn new_document_saves_utf8_lf_by_default() {
        let dir = TempDir::new("newdoc");
        let doc = Document::from_str("строка один\nстрока два\n");
        assert_eq!(doc.encoding(), Encoding::utf8());
        assert_eq!(doc.line_endings().dominant(), LineEnding::Lf);

        let dst = dir.path("new.txt");
        doc.save_file(&dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), "строка один\nстрока два\n".as_bytes());
    }

    /// Нормализатор: \r|\n через границу кусков и одинокий хвостовой CR.
    #[test]
    fn normalizer_handles_cr_across_chunk_boundary() {
        let mut n = NewlineNormalizer::new();
        assert_eq!(n.push("a\r"), "a");
        assert_eq!(n.push("\nb"), "\nb");
        assert_eq!(n.finish(), "");
        assert_eq!(n.stats(), LineEndingStats { lf: 0, crlf: 1, cr: 0 });

        let mut n2 = NewlineNormalizer::new();
        assert_eq!(n2.push("x\ry"), "x\ny");
        assert_eq!(n2.finish(), "");
        assert_eq!(n2.stats(), LineEndingStats { lf: 0, crlf: 0, cr: 1 });

        let mut n3 = NewlineNormalizer::new();
        assert_eq!(n3.push("a\nb\r"), "a\nb");
        assert_eq!(n3.finish(), "\n");
        assert_eq!(n3.stats(), LineEndingStats { lf: 1, crlf: 0, cr: 1 });
    }
}
