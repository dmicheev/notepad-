//! Поиск и замена (группа 6, D13): GPUI-free движок по `Document`
//! (ropey) и по файлам папки. Режимы: текст / regex (fancy-regex —
//! lookaround), учёт регистра, целые слова.
//!
//! ## Поиск по rope чанками (память D11.4)
//! Rope НЕ материализуется в String: чанки накапливаются в буфер до
//! `SEARCH_WINDOW` (256 КБ), regex гоняется по буферу, уверенно
//! берутся совпадения, начинающиеся до `buf.len() - SEARCH_OVERLAP`;
//! затем буфер сдвигается (хвост `SEARCH_OVERLAP` остаётся — совпадение
//! через границу чанков/окон находится в следующем окне). Остаточное
//! ограничение (зафиксировано в D13): lookbehind длиннее 1024 байт,
//! начинающийся до начала окна, может дать иное совпадение, чем полный
//! поиск по файлу одной строкой; совпадения нулевой длины (паттерн
//! `a*`) не собираются — защита от бесконечных итераций.
//!
//! ## char vs byte (ловушка задачника)
//! regex/fancy-regex отдают байтовые offsets по `&str`; Document —
//! char-индексы. Конверсия — `Rope::byte_to_char` от глобального
//! байтового начала буфера (паттерн `build_spans` в highlight.rs).
//! Вторая грань той же ловушки (поймана падением в живом окне):
//! байтовая арифметика окна (`len − SEARCH_OVERLAP`) может попастьнуть
//! ВНУТРЬ multibyte-символа — сдвиг буфера округляется вниз до char-
//! границы (`shift_window`), тест крутит документ смещениями, пока
//! граница окна не окажется посреди кириллического символа.
//!
//! ## Скан папки (6.4, D3)
//! memmap + rayon (единственные новые зависимости, разрешены задачником):
//! обход дерева руками (std::fs, симлинки пропускаются), фильтр по
//! простой маске-глобу (`*.rs`, `*.{ts,tsx}` — руками, без зависимостей),
//! бинарные файлы пропускаются (NUL в первых 8 КБ), кодировки — по
//! образцу encoding.rs: BOM (UTF-8/16) → валидация UTF-8 → chardetng
//! по префиксу 64 КБ, декодирование lossy и поиск по строке
//! (компромисс и границы — D13). Результаты — батчи через колбэк
//! (вызывается из rayon-потоков; UI-слой bridg'ует их в канал).
//!
//! ## Замена
//! Тот же проход по окнам, но `captures_iter` + `Captures::expand`
//! (подстановки `$1`/`$имя`; в текстовом режиме замена литеральная).
//! Правки отдаются как `(start_char, end_char, text)` в
//! до-координатах — app применяет их ОДНОЙ составной транзакцией
//! (undo одним шагом, D2.4). Ошибка regex (backtrack limit) — Err,
//! не паника.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fancy_regex::Regex as FancyRegex;
use ropey::Rope;

use crate::document::Document;

/// Размер окна поиска по rope (байты буфера).
const SEARCH_WINDOW: usize = 256 * 1024;
/// Хвост буфера, переносимый в следующее окно: покрывает совпадения и
/// lookbehind, пересекающие границу окон (остаточное ограничение — D13).
const SEARCH_OVERLAP: usize = 1024;
/// Префикс файла для детекта бинарности (NUL в нём → файл пропускается).
const BINARY_SNIFF: usize = 8 * 1024;
/// Префикс для кормления chardetng при не-UTF-8 файле (по образцу
/// encoding.rs — CHUNK там 64 КБ).
const DETECT_PREFIX: usize = 64 * 1024;
/// Максимум char в фрагменте результата (UI панели SEARCH).
pub const FRAGMENT_MAX_CHARS: usize = 160;
/// Отступ фрагмента до совпадения (char).
const FRAGMENT_LEAD: usize = 30;

/// Режимы поиска.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    /// Учитывать регистр (по умолчанию — нет).
    pub case_sensitive: bool,
    /// Только целые слова (границы `\b` на word-краях паттерна).
    pub whole_word: bool,
    /// Паттерн — регулярное выражение (fancy-regex, lookaround).
    pub regex: bool,
}

/// Скомпилированный поисковик: один экземпляр на (паттерн, опции).
#[derive(Debug)]
pub struct SearchMatcher {
    opts: SearchOptions,
    pattern: String,
    re: FancyRegex,
}

impl SearchMatcher {
    /// Компилирует паттерн; текстовый режим экранируется
    /// (`fancy_regex::escape`). Ошибка компиляции — Err со строкой
    /// (UI показывает в статус-баре, не паникует).
    pub fn new(pattern: &str, opts: SearchOptions) -> Result<Self, String> {
        let body = if opts.regex {
            pattern.to_string()
        } else {
            fancy_regex::escape(pattern).into_owned()
        };
        // Целые слова: \b нужен только на word-краях паттерна (поиск
        // "..." с \b вообще не матчится); в regex-режиме граница
        // добавляется по краям ВСЕГО паттерна (не каждой альтернативы).
        // Регистр — ИНЛАЙН-флагом `(?i)`: опция RegexBuilder::
        // case_insensitive в fancy-regex 0.14 не пробрасывается
        // в делегированный regex crate (проверено мини-стендом).
        let (lead, tail) = word_boundaries(&body, opts);
        let case_flag = if opts.case_sensitive { "" } else { "(?i)" };
        let wrapped = format!("{case_flag}{lead}{body}{tail}");
        let re = fancy_regex::RegexBuilder::new(&wrapped)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(SearchMatcher { opts, pattern: pattern.to_string(), re })
    }

    pub fn options(&self) -> SearchOptions {
        self.opts
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Быстрая проверка, что паттерн вообще скомпилирован (для тестов).
    pub fn is_regex(&self) -> bool {
        self.opts.regex
    }

    /// Все совпадения в документе — char-диапазоны `[start, end)`,
    /// отсортированные, без нулевой длины.
    pub fn find_in_doc(&self, doc: &Document) -> Vec<(usize, usize)> {
        self.find_in_rope(doc.rope())
    }

    /// Совпадения, начиная с char-позиции `from` (инкрементальная
    /// инвалидация панели при правках, D13): окно открывается с
    /// отступом `SEARCH_OVERLAP` назад — покрывает lookbehind.
    pub fn find_in_doc_from(&self, doc: &Document, from: usize) -> Vec<(usize, usize)> {
        let rope = doc.rope();
        let from = from.min(doc.len_chars());
        let from_byte = rope.char_to_byte(from);
        let start_byte = from_byte.saturating_sub(SEARCH_OVERLAP);
        let mut out = Vec::new();
        let (mut chunks, byte, _char, _line) = rope.chunks_at_byte(start_byte);
        let mut state = WindowState { buf: String::new(), buf_byte_start: byte };
        for chunk in &mut chunks {
            state.buf.push_str(chunk);
            if state.buf.len() >= SEARCH_WINDOW {
                drain_window(rope, &self.re, &mut state, &mut out, false);
            }
        }
        if !state.buf.is_empty() {
            drain_window(rope, &self.re, &mut state, &mut out, true);
        }
        // Отчистить совпадения, начавшиеся до `from` (зона overlap):
        // они принадлежат предыдущему (неинвалидированному) куску.
        out.retain(|&(s, _)| s >= from);
        out
    }

    /// Совпадения в строке (скан файлов, поиск по вкладкам из app
    /// использует find_in_doc) — char-диапазоны.
    pub fn find_in_str(&self, text: &str) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for m in self.re.find_iter(text) {
            let Ok(m) = m else { break };
            if m.start() == m.end() {
                continue;
            }
            out.push((
                text[..m.start()].chars().count(),
                text[..m.end()].chars().count(),
            ));
        }
        out
    }

    /// Замены по документу: `(start_char, end_char, текст_замены)` в
    /// до-координатах, одна правка на совпадение. Regex-режим —
    /// `Captures::expand` (`$1`/`$имя`); текстовый — литеральная строка.
    /// Ошибка движка (backtrack limit) — Err.
    pub fn replacements_in_doc(
        &self,
        doc: &Document,
        replacement: &str,
    ) -> Result<Vec<(usize, usize, String)>, String> {
        let rope = doc.rope();
        let mut out = Vec::new();
        let mut state = WindowState { buf: String::new(), buf_byte_start: 0 };
        for chunk in rope.chunks() {
            state.buf.push_str(chunk);
            if state.buf.len() >= SEARCH_WINDOW {
                drain_replacements(rope, &self.re, self.opts.regex, replacement, &mut state, &mut out, false)?;
            }
        }
        if !state.buf.is_empty() {
            drain_replacements(rope, &self.re, self.opts.regex, replacement, &mut state, &mut out, true)?;
        }
        Ok(out)
    }
}

/// Word-границы для whole_word: `\b` только там, где край паттерна —
/// word-символ (поиск "..." или "(x)" с безусловным \b не матчится).
fn word_boundaries(body: &str, opts: SearchOptions) -> (String, String) {
    if !opts.whole_word {
        return (String::new(), String::new());
    }
    let lead = if body
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_') { r"\b".to_string() } else { Default::default() };
    let tail = if body
        .chars()
        .next_back()
        .is_some_and(|c| c.is_alphanumeric() || c == '_') { r"\b".to_string() } else { Default::default() };
    (lead, tail)
}

/// Состояние оконного прохода по rope.
struct WindowState {
    buf: String,
    /// Глобальный байтовый offset начала `buf` в rope.
    buf_byte_start: usize,
}

/// Ищет совпадения в буфере, начинающиеся до `cutoff` (последнее окно —
/// до конца), добавляет их в `out` (char-координаты rope), сдвигает
/// буфер (хвост SEARCH_OVERLAP переносится).
fn drain_window(
    rope: &Rope,
    re: &FancyRegex,
    state: &mut WindowState,
    out: &mut Vec<(usize, usize)>,
    last: bool,
) {
    let cutoff = if last {
        state.buf.len()
    } else {
        state.buf.len().saturating_sub(SEARCH_OVERLAP)
    };
    for m in re.find_iter(&state.buf) {
        let Ok(m) = m else { break };
        if m.start() >= cutoff {
            break; // итератор отсортирован — дальше тоже за cutoff
        }
        if m.start() == m.end() {
            continue; // нулевые совпадения не собираем (D13)
        }
        let s = rope.byte_to_char(state.buf_byte_start + m.start());
        let e = rope.byte_to_char(state.buf_byte_start + m.end());
        out.push((s, e));
    }
    shift_window(state, cutoff);
}

/// Вариант drain для замен: captures + expand. Отсечение по `cutoff`
/// симметрично `drain_window` — совпадения из зоны overlap попадут в
/// следующее окно (без дубликатов).
fn drain_replacements(
    rope: &Rope,
    re: &FancyRegex,
    is_regex: bool,
    replacement: &str,
    state: &mut WindowState,
    out: &mut Vec<(usize, usize, String)>,
    last: bool,
) -> Result<(), String> {
    let cutoff = if last {
        state.buf.len()
    } else {
        state.buf.len().saturating_sub(SEARCH_OVERLAP)
    };
    for caps in re.captures_iter(&state.buf) {
        let caps = caps.map_err(|e| e.to_string())?;
        let m = caps.get(0).expect("group 0 is the whole match");
        if m.start() >= cutoff {
            break;
        }
        if m.start() == m.end() {
            continue;
        }
        let text = if is_regex {
            let mut dst = String::new();
            caps.expand(replacement, &mut dst);
            dst
        } else {
            replacement.to_string()
        };
        let s = rope.byte_to_char(state.buf_byte_start + m.start());
        let e = rope.byte_to_char(state.buf_byte_start + m.end());
        out.push((s, e, text));
    }
    shift_window(state, cutoff);
    Ok(())
}

/// Оставить хвост буфера начиная с `keep_from` (байты). `keep_from`
/// обязан попадать на char-границу: совпадения regex — тоже на
/// границах, но `len − SEARCH_OVERLAP` (чисто байтовая арифметика)
/// может попастьнуть ВНУТРЬ multibyte-символа — округляем вниз до
/// ближайшей границы (ловушка поймана на кириллице падением в живом
/// окне: «start byte index is not a char boundary», search.rs:298).
fn shift_window(state: &mut WindowState, keep_from: usize) {
    let mut keep_from = keep_from.min(state.buf.len());
    while keep_from > 0 && !state.buf.is_char_boundary(keep_from) {
        keep_from -= 1;
    }
    if keep_from >= state.buf.len() {
        state.buf.clear();
    } else {
        let tail = state.buf[keep_from..].to_string();
        state.buf.clear();
        state.buf.push_str(&tail);
    }
    state.buf_byte_start += keep_from;
}

impl SearchMatcher {
    /// Все совпадения по rope (окна SEARCH_WINDOW с перекрытием).
    /// Публичный: фоновый поиск по вкладкам app-слоя работает со
    /// склонированными ropes (Document не покидает модель вкладок).
    pub fn find_in_rope(&self, rope: &Rope) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut state = WindowState { buf: String::new(), buf_byte_start: 0 };
        for chunk in rope.chunks() {
            state.buf.push_str(chunk);
            if state.buf.len() >= SEARCH_WINDOW {
                drain_window(rope, &self.re, &mut state, &mut out, false);
            }
        }
        if !state.buf.is_empty() {
            drain_window(rope, &self.re, &mut state, &mut out, true);
        }
        out
    }
}

// ---- Скан папки (6.4) ----

/// Одно совпадение в файле: строка/колонка 0-based (char), длина в
/// char, фрагмент строки с позицией совпадения внутри него.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHit {
    pub line: usize,
    pub col: usize,
    pub len: usize,
    /// Фрагмент строки (обрезан до `FRAGMENT_MAX_CHARS`).
    pub fragment: String,
    /// Позиция совпадения внутри фрагмента (char).
    pub frag_col: usize,
    /// Длина совпадения внутри фрагмента (может быть обрезана).
    pub frag_len: usize,
}

/// Результаты скана одного файла.
#[derive(Clone, Debug)]
pub struct FileScan {
    pub path: PathBuf,
    pub hits: Vec<FileHit>,
}

/// Итог скана папки.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanSummary {
    /// Просканировано файлов (бинарные/нечитаемые не в счёте).
    pub files_scanned: usize,
    /// Пропущено (бинарные с NUL / ошибки чтения).
    pub files_skipped: usize,
    /// Файлов с совпадениями.
    pub files_with_hits: usize,
    /// Всего совпадений.
    pub total_hits: usize,
}

/// Матч простой маски-глоба к ИМЕНИ файла: `*` (любая последовательность),
/// `?` (один символ), `{a,b}` — альтернативы (одна вложенность).
/// Регистронезависимо (ASCII) — Windows-привычка, зафиксировано в D13.
/// Пустая маска — все файлы.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    if pattern.trim().is_empty() {
        return true;
    }
    expand_braces(pattern).iter().any(|p| match_one(&p.to_lowercase(), &name.to_lowercase()))
}

/// Развернуть `{a,b,c}` в альтернативы (рекурсивно по первому уровню).
fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    // Найти парную закрывающую (учёт вложенности).
    let mut depth = 0;
    let mut close = None;
    for (i, c) in pattern[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return vec![pattern.to_string()]; // непарная — литерал
    };
    let prefix = &pattern[..open];
    let body = &pattern[open + 1..close];
    let suffix = &pattern[close + 1..];
    // Тело: сплит по ',' верхнего уровня (вложенные {} внутри варианта
    // развернутся рекурсией).
    let mut variants = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in body.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                variants.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    variants.push(&body[start..]);
    variants
        .into_iter()
        .flat_map(|v| expand_braces(&format!("{prefix}{v}{suffix}")))
        .collect()
}

/// Классический glob-матч одного паттерна (без {}): `*`, `?`.
fn match_one(pat: &str, name: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let n: Vec<char> = name.chars().collect();
    // Итеративный матч с бэктрекингом по '*'.
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut star_n) = (None::<usize>, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            star_n = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            star_n += 1;
            ni = star_n;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Собрать файлы папки (рекурсивно, с подпапками), отфильтрованные по
/// маске-глобу к имени. Симлинки пропускаются (циклы), сортировка —
/// детерминизм. Ошибка чтения подпапки — Err.
pub fn collect_files(root: &Path, mask: &str) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            if ft.is_symlink() {
                continue;
            }
            let path = entry.path();
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if glob_match(mask, &name) {
                    out.push(path);
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Просканировать один файл: memmap, NUL-детект, кодировки (BOM →
/// UTF-8 валидация → chardetng), поиск, фрагменты. `None` — бинарный
/// или нечитаемый файл (пропуск).
pub fn scan_file(path: &Path, matcher: &SearchMatcher) -> Option<FileScan> {
    let file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None; // пустые файлы неинтересны
    }
    // safety: mmap валиден, пока жив file; мы читаем синхронно и не
    // мутируем файл (запись пользователем в этот момент — вне контракта).
    let mmap = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    let bytes = &mmap[..];
    // BOM-файлы (UTF-16) содержат NUL-байты легально — бинарный детект
    // только для файлов без BOM (порядок важен, ловушка поймана тестом).
    let has_bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF])
        || bytes.starts_with(&[0xFF, 0xFE])
        || bytes.starts_with(&[0xFE, 0xFF]);
    if !has_bom && bytes[..bytes.len().min(BINARY_SNIFF)].contains(&0) {
        return None; // бинарный
    }
    let text = decode_for_search(bytes)?;
    let mut hits = Vec::new();
    // Построчная раскладка: совпадения отсортированы, строка и начало
    // её байт ведутся инкрементально.
    let mut line: usize = 0;
    let mut line_byte_start: usize = 0;
    for m in matcher.re.find_iter(&text) {
        let Ok(m) = m else { break };
        if m.start() == m.end() {
            continue;
        }
        // Продвинуть строку до совпадения.
        while let Some(nl) = text[line_byte_start..m.start()].find('\n') {
            line_byte_start += nl + 1;
            line += 1;
        }
        // Конец строки (терминатор не входит).
        let line_byte_end = text[m.start()..]
            .find('\n')
            .map(|i| m.start() + i)
            .unwrap_or(text.len())
            .max(line_byte_start);
        // Колонка (char) от начала строки.
        let col = text[line_byte_start..m.start()].chars().count();
        let len = text[m.start()..m.end()].chars().count();
        // Фрагмент: окно вокруг совпадения внутри строки.
        let line_text = &text[line_byte_start..line_byte_end];
        let line_chars = line_text.chars().count();
        let frag_start = col.saturating_sub(FRAGMENT_LEAD);
        let frag_end = (col + len + FRAGMENT_MAX_CHARS).min(line_chars);
        let frag_start_byte = char_to_byte(line_text, frag_start);
        let frag_end_byte = char_to_byte(line_text, frag_end);
        let fragment = line_text[frag_start_byte..frag_end_byte].to_string();
        let frag_col = col - frag_start;
        let frag_len = len.min(frag_end - col);
        hits.push(FileHit {
            line,
            col,
            len,
            fragment,
            frag_col,
            frag_len,
        });
    }
    Some(FileScan { path: path.to_path_buf(), hits })
}

/// char-позиция → байтовая в строке.
fn char_to_byte(s: &str, chars: usize) -> usize {
    s.char_indices().nth(chars).map(|(b, _)| b).unwrap_or(s.len())
}

/// Декодирование байт для поиска: UTF-8 (валидно) как есть; BOM —
/// фиксированные UTF-8/16; иначе chardetng по префиксу, lossy-декод.
fn decode_for_search(bytes: &[u8]) -> Option<String> {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Some(s.to_string());
    }
    // BOM (по образцу encoding.rs).
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        let (s, _, _) = encoding_rs::UTF_8.decode(bytes);
        return Some(s.into_owned());
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let (s, _, _) = encoding_rs::UTF_16LE.decode(bytes);
        return Some(s.into_owned());
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let (s, _, _) = encoding_rs::UTF_16BE.decode(bytes);
        return Some(s.into_owned());
    }
    // chardetng по префиксу ( DETECT_PREFIX достаточно — компромисс:
    // для очень коротких экзотических файлов возможна ошибка кодировки,
    // поиск тогда по lossy-строке; границы — D13).
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(&bytes[..bytes.len().min(DETECT_PREFIX)], true);
    let enc = detector.guess(None, true);
    let (s, _, _) = enc.decode(bytes);
    Some(s.into_owned())
}

/// Параллельный скан списка файлов (rayon, чанки по 16 файлов — батчи
/// для потокового UI). `on_batch` вызывается из rayon-потоков: только
/// Send+Sync работа (канал в app-слое). Порядок файлов внутри батча
/// сохранён, порядок батчей — нет (UI сортирует при вставке).
pub fn scan_paths(
    paths: &[PathBuf],
    matcher: &SearchMatcher,
    on_batch: &(dyn Fn(Vec<FileScan>) + Send + Sync),
) -> ScanSummary {
    use rayon::prelude::*;
    let scanned = std::sync::atomic::AtomicUsize::new(0);
    let skipped = std::sync::atomic::AtomicUsize::new(0);
    let with_hits = std::sync::atomic::AtomicUsize::new(0);
    let total_hits = std::sync::atomic::AtomicUsize::new(0);
    use std::sync::atomic::Ordering;
    paths.par_chunks(16).for_each(|chunk| {
        let mut batch = Vec::new();
        for path in chunk {
            match scan_file(path, matcher) {
                Some(scan) => {
                    scanned.fetch_add(1, Ordering::Relaxed);
                    if !scan.hits.is_empty() {
                        with_hits.fetch_add(1, Ordering::Relaxed);
                        total_hits.fetch_add(scan.hits.len(), Ordering::Relaxed);
                    }
                    batch.push(scan);
                }
                None => {
                    skipped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        on_batch(batch);
    });
    ScanSummary {
        files_scanned: scanned.into_inner(),
        files_skipped: skipped.into_inner(),
        files_with_hits: with_hits.into_inner(),
        total_hits: total_hits.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use crate::transaction::Transaction;
    use std::io::Write;

    fn opts(regex: bool, case: bool, word: bool) -> SearchOptions {
        SearchOptions { case_sensitive: case, whole_word: word, regex }
    }

    fn find(doc: &Document, pattern: &str, o: SearchOptions) -> Vec<(usize, usize)> {
        SearchMatcher::new(pattern, o).unwrap().find_in_doc(doc)
    }

    // ---- Режимы (текст/регистр/целые слова/regex) ----

    #[test]
    fn text_find_case_and_whole_word() {
        // "cat catalog CAT scat": cat(0) catalog(4) CAT(12) scat(16),
        // «cat» внутри scat — с 17.
        let doc = Document::from_text("cat catalog CAT scat\n");
        assert_eq!(
            find(&doc, "cat", opts(false, false, false)),
            vec![(0, 3), (4, 7), (12, 15), (17, 20)]
        );
        // Учёт регистра: «cat» в catalog/scat остаётся, «CAT» нет.
        assert_eq!(find(&doc, "cat", opts(false, true, false)), vec![(0, 3), (4, 7), (17, 20)]);
        // Целые слова: только «cat» и «CAT».
        assert_eq!(find(&doc, "cat", opts(false, false, true)), vec![(0, 3), (12, 15)]);
        // Учёт регистра + целые слова.
        assert_eq!(find(&doc, "cat", opts(false, true, true)), vec![(0, 3)]);
    }

    #[test]
    fn whole_word_with_punctuation_edges() {
        // Край паттерна — не word-символ: \b не добавляется (иначе
        // совпадений нет вовсе).
        let doc = Document::from_text("f(x) + f(y)\n");
        assert_eq!(find(&doc, "f(", opts(false, false, true)), vec![(0, 2), (7, 9)]);
    }

    #[test]
    fn multibyte_char_positions() {
        // char vs byte (ловушка): «привет» — 12 байт, 6 char.
        // «привет мир привет»: второй «привет» с char 11 (6+1+3+1).
        let doc = Document::from_text("привет мир привет\n");
        assert_eq!(find(&doc, "привет", opts(false, false, false)), vec![(0, 6), (11, 17)]);
    }

    /// Регрессия падения из живого окна: сдвиг окна `len −
    /// SEARCH_OVERLAP` в БАЙТАХ попадал внутрь кириллического символа —
    /// паника среза в `shift_window`. Крутим префиксы: какая-то из
    /// границ окна обязательно оказывается посреди multibyte-символа.
    #[test]
    fn multibyte_window_shift_never_splits_chars() {
        let mut body = String::new();
        while body.len() < 300 * 1024 {
            body.push_str("привет мир needle хвост ");
            body.push_str("ещё текст ");
        }
        for shift in 0..48usize {
            let mut text = "x".repeat(shift);
            text.push_str(&body);
            let doc = Document::from_text(&text);
            // Эталон — поиск по цельной строке (char-диапазоны).
            let expected = SearchMatcher::new("needle", opts(false, false, false))
                .unwrap()
                .find_in_str(&text);
            let found = find(&doc, "needle", opts(false, false, false));
            assert_eq!(found, expected, "prefix shift={shift}");
            // Путь замен использует ту же оконную механику.
            let m = SearchMatcher::new("мир", opts(false, false, false)).unwrap();
            let edits = m.replacements_in_doc(&doc, "ХАН").unwrap();
            let found_mir = find(&doc, "мир", opts(false, false, false));
            assert_eq!(edits.len(), found_mir.len(), "replacements path, shift={shift}");
        }
    }

    #[test]
    fn regex_dates_scenario() {
        // Сценарий спеки: все даты YYYY-MM-DD.
        let doc = Document::from_text("due 2024-01-31 and 2023-12-01\nnot 2024-1-31\n");
        assert_eq!(
            find(&doc, r"\d{4}-\d{2}-\d{2}", opts(true, false, false)),
            vec![(4, 14), (19, 29)]
        );
    }

    #[test]
    fn regex_error_is_err_not_panic() {
        assert!(SearchMatcher::new("(unclosed", opts(true, false, false)).is_err());
        // Пустой паттерн в regex-режиме компилируется, но нулевые
        // совпадения не собираются.
        let doc = Document::from_text("ab");
        assert!(find(&doc, "", opts(false, false, false)).is_empty());
    }

    #[test]
    fn empty_matches_are_skipped() {
        let doc = Document::from_text("hello world\n");
        assert!(find(&doc, "x*", opts(true, false, false)).is_empty());
    }

    // ---- Окна/чанки ----

    #[test]
    fn match_across_chunk_window_boundary() {
        // Документ > SEARCH_WINDOW: совпадение ровно на границе окон
        // находится; эталон — поиск по цельной строке.
        let mut text = String::new();
        // Заполняем: 300 КБ, «needle» каждые 4096 байт + одно ровно
        // на границе 256 КБ (SEARCH_WINDOW) минус 3 байта (начало
        // совпадения до границы, конец после).
        while text.len() < 300 * 1024 {
            let pad = text.len();
            let target = if (256 * 1024 - 3) > pad && (256 * 1024 - 3) - pad < 4096 {
                (256 * 1024 - 3) - pad // needle начнётся ровно на 256К-3
            } else {
                4096
            };
            for _ in 0..target {
                text.push('a');
            }
            text.push_str("needle");
        }
        let doc = Document::from_text(&text);
        let found = find(&doc, "needle", opts(false, false, false));
        // Эталон: str::find по цельной строке.
        let mut expected = Vec::new();
        let mut from = 0;
        while let Some(i) = text[from..].find("needle") {
            let byte = from + i;
            let s = text[..byte].chars().count();
            expected.push((s, s + 6));
            from = byte + 6;
        }
        assert_eq!(found, expected);
        // Совпадение через границу окна присутствует.
        let boundary_char = text[..256 * 1024 - 3].chars().count();
        assert!(found.contains(&(boundary_char, boundary_char + 6)));
    }

    #[test]
    fn find_from_matches_suffix_of_full_search() {
        let text = "alpha beta alpha beta alpha\n";
        let doc = Document::from_text(text);
        let m = SearchMatcher::new("alpha", opts(false, false, false)).unwrap();
        let full = m.find_in_doc(&doc);
        for from in 0..doc.len_chars() + 1 {
            let partial = m.find_in_doc_from(&doc, from);
            let expected: Vec<_> = full.iter().copied().filter(|&(s, _)| s >= from).collect();
            assert_eq!(partial, expected, "from={from}");
        }
    }

    #[test]
    fn rope_built_from_many_chunks_matches_string() {
        // RopeBuilder из мелких кусков — чанки rope мелкие; поиск
        // идентичен поиску по строке.
        let mut builder = ropey::RopeBuilder::new();
        let mut expected = Vec::new();
        let mut chars = 0;
        for i in 0..500 {
            let piece = if i % 7 == 0 { "XabcX " } else { "filler " };
            if let Some(pos) = piece.find("abc") {
                expected.push((chars + pos, chars + pos + 3));
            }
            chars += piece.chars().count();
            builder.append(piece);
        }
        let rope = builder.finish();
        let doc = Document::from_rope(rope);
        assert_eq!(find(&doc, "abc", opts(false, false, false)), expected);
    }

    // ---- Замена ----

    #[test]
    fn replace_all_text_mode() {
        let doc = Document::from_text("cat dog cat\ncat\n");
        let m = SearchMatcher::new("cat", opts(false, false, false)).unwrap();
        let edits = m.replacements_in_doc(&doc, "wolf").unwrap();
        let mut doc = doc;
        doc.apply(&Transaction::from_edits(
            edits
                .into_iter()
                .map(|(s, e, t)| crate::transaction::Edit { start: s, end: e, text: t })
                .collect(),
        ));
        assert_eq!(doc.text(), "wolf dog wolf\nwolf\n");
    }

    #[test]
    fn replace_all_with_capture_group_scenario() {
        // Сценарий спеки: (\w+)@example\.com → <$1>.
        let doc = Document::from_text("alice@example.com bob@example.com x@other.ru\n");
        let m = SearchMatcher::new(r"(\w+)@example\.com", opts(true, false, false)).unwrap();
        let edits = m.replacements_in_doc(&doc, "<$1>").unwrap();
        assert_eq!(edits.len(), 2);
        let mut doc = doc;
        doc.apply(&Transaction::from_edits(
            edits
                .into_iter()
                .map(|(s, e, t)| crate::transaction::Edit { start: s, end: e, text: t })
                .collect(),
        ));
        assert_eq!(doc.text(), "<alice> <bob> x@other.ru\n");
    }

    #[test]
    fn text_mode_replacement_is_literal() {
        // $1 в текстовом режиме — литерал.
        let doc = Document::from_text("a b a\n");
        let m = SearchMatcher::new("a", opts(false, false, false)).unwrap();
        let edits = m.replacements_in_doc(&doc, "$1x").unwrap();
        let mut doc = doc;
        doc.apply(&Transaction::from_edits(
            edits
                .into_iter()
                .map(|(s, e, t)| crate::transaction::Edit { start: s, end: e, text: t })
                .collect(),
        ));
        assert_eq!(doc.text(), "$1x b $1x\n");
    }

    #[test]
    fn sequential_single_replaces_shift_correctly() {
        // Replace по одному: после каждой правки пересчёт замен —
        // следующий Replacement применяется к НОВОМУ документу
        // (решение «пересчёт после каждой правки», D13).
        let mut doc = Document::from_text("aa bb aa bb aa\n");
        for remaining in (1..=3).rev() {
            let m = SearchMatcher::new("aa", opts(false, false, false)).unwrap();
            let mut edits = m.replacements_in_doc(&doc, "X").unwrap();
            assert_eq!(edits.len(), remaining, "осталось {remaining} вхождений");
            let one = edits.drain(..1).collect::<Vec<_>>();
            doc.apply(&Transaction::from_edits(
                one.into_iter()
                    .map(|(s, e, t)| crate::transaction::Edit { start: s, end: e, text: t })
                    .collect(),
            ));
        }
        assert_eq!(doc.text(), "X bb X bb X\n");
    }

    #[test]
    fn undo_after_replace_all_restores_document() {
        let mut doc = Document::from_text("one two one two one\n");
        let mut history = crate::undo::History::new();
        let m = SearchMatcher::new("one", opts(false, false, false)).unwrap();
        let edits = m.replacements_in_doc(&doc, "1").unwrap();
        let tx = Transaction::from_edits(
            edits
                .into_iter()
                .map(|(s, e, t)| crate::transaction::Edit { start: s, end: e, text: t })
                .collect(),
        );
        history.apply(&mut doc, &tx);
        assert_eq!(doc.text(), "1 two 1 two 1\n");
        assert!(history.undo(&mut doc));
        assert_eq!(doc.text(), "one two one two one\n");
    }

    // ---- Глоб ----

    #[test]
    fn glob_simple_and_braces() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "main.rs.bak"));
        assert!(glob_match("*.{ts,tsx}", "app.tsx"));
        assert!(glob_match("*.{ts,tsx}", "app.ts"));
        assert!(!glob_match("*.{ts,tsx}", "app.js"));
        assert!(glob_match("test-?.txt", "test-1.txt"));
        assert!(!glob_match("test-?.txt", "test-10.txt"));
        // Регистронезависимо (ASCII).
        assert!(glob_match("*.RS", "main.Rs"));
        // Пустая маска — все.
        assert!(glob_match("", "anything.bin"));
        // Непарная скобка — литерал.
        assert!(glob_match("a{b", "a{b"));
    }

    #[test]
    fn collect_files_recursive_filtered() {
        let dir = TempDir::new("search-collect");
        dir.write("a.rs", "x");
        dir.write("b.txt", "x");
        let sub = dir.path("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("c.rs"), "x").unwrap();
        std::fs::write(sub.join("d.js"), "x").unwrap();
        let root = dir.path("");

        let all = collect_files(&root, "").unwrap();
        assert_eq!(all.len(), 4);
        let rs = collect_files(&root, "*.rs").unwrap();
        assert_eq!(rs.len(), 2);
        assert!(rs.iter().all(|p| p.extension().is_some_and(|e| e == "rs")));
    }

    // ---- Скан файлов ----

    #[test]
    fn scan_file_lines_fragments_encodings() {
        let dir = TempDir::new("search-scan");
        // UTF-8: две строки с совпадениями.
        let p8 = dir.write("u8.txt", "alpha needle here\nnothing\nsecond needle line\n");
        let m = SearchMatcher::new("needle", opts(false, false, false)).unwrap();
        let scan = scan_file(&p8, &m).expect("utf8 scan");
        assert_eq!(scan.hits.len(), 2);
        assert_eq!(scan.hits[0].line, 0);
        assert_eq!(scan.hits[0].col, 6);
        assert_eq!(scan.hits[0].len, 6);
        assert!(scan.hits[0].fragment.contains("needle"));
        assert!(scan.hits[0].fragment.starts_with("alpha"));
        assert_eq!(scan.hits[1].line, 2);

        // windows-1251 (кириллица) — декодирование и поиск.
        let p1251 = dir.path("w1251.txt");
        let (encoded, _, _) = encoding_rs::WINDOWS_1251.encode("привет мир\nещё привет\n");
        std::fs::write(&p1251, &encoded).unwrap();
        let mc = SearchMatcher::new("привет", opts(false, false, false)).unwrap();
        let scan = scan_file(&p1251, &mc).expect("1251 scan");
        assert_eq!(scan.hits.len(), 2, "не-UTF-8 декодируется и ищется");
        assert_eq!(scan.hits[1].line, 1);

        // UTF-16LE с BOM.
        let p16 = dir.path("u16.txt");
        let text = "needle utf16\nplain\n";
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(&p16, &bytes).unwrap();
        let scan = scan_file(&p16, &m).expect("utf16 scan");
        assert_eq!(scan.hits.len(), 1);
        assert_eq!(scan.hits[0].line, 0);

        // Бинарный (NUL) — пропуск.
        let pbin = dir.path("bin.dat");
        std::fs::write(&pbin, b"needle\x00binary").unwrap();
        assert!(scan_file(&pbin, &m).is_none());

        // Мультибайт-колонки: фрагмент и col в char.
        let pm = dir.write("mb.txt", "ωωω needle\n");
        let scan = scan_file(&pm, &m).expect("mb scan");
        assert_eq!(scan.hits[0].col, 4, "col в char, не байтах");
    }

    #[test]
    fn scan_paths_batches_and_summary() {
        let dir = TempDir::new("search-paths");
        let mut paths = Vec::new();
        for i in 0..40 {
            let p = dir.write(&format!("f{i:02}.txt"), if i % 2 == 0 { "hit here\n" } else { "miss\n" });
            paths.push(p);
        }
        paths.push(dir.write("bin.bin", "x\0y"));
        let m = SearchMatcher::new("hit", opts(false, false, false)).unwrap();
        let batches: std::sync::Mutex<Vec<Vec<FileScan>>> = std::sync::Mutex::new(Vec::new());
        let summary = scan_paths(&paths, &m, &|batch| {
            batches.lock().unwrap().push(batch);
        });
        assert_eq!(summary.files_with_hits, 20);
        assert_eq!(summary.total_hits, 20);
        assert_eq!(summary.files_skipped, 1);
        assert_eq!(summary.files_scanned, 40);
        let all: Vec<_> = batches.into_inner().unwrap().into_iter().flatten().collect();
        assert_eq!(all.len(), 40);
    }

    #[test]
    fn scan_hundred_files_under_two_seconds() {
        // Производительность (6.4): 100 файлов < 2 с (замер в release
        // актуален; debug-порог с запасом в 10x — 20 с).
        let dir = TempDir::new("search-perf");
        let mut paths = Vec::new();
        for i in 0..100 {
            let mut content = String::new();
            for l in 0..200 {
                content.push_str(&format!("line {l} of file {i} filler words here\n"));
                if l % 20 == 0 {
                    content.push_str("TARGET marker line\n");
                }
            }
            paths.push(dir.write(&format!("p{i:03}.txt"), &content));
        }
        let m = SearchMatcher::new("TARGET", opts(false, false, false)).unwrap();
        let started = std::time::Instant::now();
        let summary = scan_paths(&paths, &m, &|_| {});
        let elapsed = started.elapsed();
        assert_eq!(summary.total_hits, 1000);
        assert!(
            elapsed < std::time::Duration::from_secs(if cfg!(debug_assertions) { 20 } else { 2 }),
            "100 файлов просканированы за {:?}",
            elapsed
        );
    }

    // Скан файлов с lookup: убедиться, что write закрывает файлы
    // (mmap на Windows требует закрытого хендла записи).
    #[test]
    fn scan_file_after_write_closes() {
        let dir = TempDir::new("search-close");
        let p = dir.write("c.txt", "needle\n");
        let m = SearchMatcher::new("needle", opts(false, false, false)).unwrap();
        assert!(scan_file(&p, &m).is_some());
        let mut f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        writeln!(f, "again needle").unwrap();
    }
}
