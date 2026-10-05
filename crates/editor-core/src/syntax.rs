//! Определение языка (задача 5.1): глобальный ленивый `SyntaxSet`
//! syntect (OnceLock — НЕ на пути старта приложения, бюджет холодного
//! старта 9.2) + чистые функции поиска синтаксиса по расширению,
//! первой строке и имени. Имена синтаксисов — источник для статус-бара
//! (app/language.rs — обёртка с friendly-именами) и ручного выбора.
//!
//! Дополнения к набору syntect 5.3 по умолчанию (75 синтаксисов, в нём
//! НЕТ TypeScript и TOML — D12.3):
//! - **TOML** — встроенная минимальная sublime-syntax-грамматика
//!   (комментарии/ключи/таблицы/строки/числа/булевы): Cargo.toml и
//!   прочие конфиги подсвечиваются;
//! - **TypeScript** — псевдоним на JavaScript-грамматику (TS —
//!   надмножество JS; типы красятся неточно, базовые конструкции —
//!   корректно). Детект возвращает грамматику + метку «TypeScript».
//!
//! Ловушка (D12): набор `load_defaults_newlines` ждёт строки С
//! терминатором — движок подсветки (highlight.rs) кормит `line_str`
//! с гарантированным `\n` на хвосте.

use std::path::Path;
use std::sync::OnceLock;

use syntect::parsing::{SyntaxDefinition, SyntaxReference, SyntaxSet};

/// Глобальный набор синтаксисов; первая инициализация (~сотни мс —
/// десериализация + постройка) — на вызывающем: в app это отложенный
/// прогрев после первого кадра, НЕ критический путь старта/открытия.
static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();

/// Расширения TypeScript: грамматика JS (в наборе нет TS — D12.3).
const TYPESCRIPT_EXTS: &[&str] = &["ts", "tsx", "mts", "cts"];

/// Минимальная грамматика TOML (в наборе по умолчанию нет). Скоупы
/// подобраны под классификатор highlight.rs: entity.name → TYPE
/// (таблицы/ключи), string/constant.numeric/constant.language/
/// comment — стандартные.
const TOML_SYNTAX: &str = r#"
%YAML 1.2
---
name: TOML
file_extensions: [toml, tml]
scope: source.toml
contexts:
  main:
    - match: '#[^\n]*'
      scope: comment.line.number-sign.toml
    - match: '^\s*\[\['
      push: table_array
    - match: '^\s*\['
      push: table
    - match: '^\s*[A-Za-z0-9_\-."]+(?=\s*=\s*)'
      scope: entity.name.tag.key.toml
    - match: '"'
      push: string
    - match: "'"
      push: literal_string
    - match: '\b(?:true|false)\b'
      scope: constant.language.toml
    - match: '\b\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?\b'
      scope: constant.numeric.toml
  table:
    - meta_scope: entity.name.section.toml
    - match: '\]\]'
      pop: true
    - match: '\]'
      pop: true
  table_array:
    - meta_scope: entity.name.section.toml
    - match: '\]\]'
      pop: true
  string:
    - meta_scope: string.quoted.double.toml
    - match: '\\.'
      scope: constant.character.escape.toml
    - match: '"'
      pop: true
  literal_string:
    - meta_scope: string.quoted.single.toml
    - match: "'"
      pop: true
"#;

/// Грамматика-заглушка CSV: реальные цветовые диапазоны строит движок
/// (`highlight::csv_spans`) — колонки чередуются цветами, sublime-
/// грамматика не умеет индексные цвета. Заглушка нужна для детекта по
/// расширению (csv/tsv) и имени в списке ручного выбора.
const CSV_SYNTAX: &str = r#"
%YAML 1.2
---
name: CSV
file_extensions: [csv, tsv]
scope: source.csv
contexts:
  main:
    - match: '.'
      scope: meta.cell.csv
"#;

/// Набор синтаксисов, инициализируя при первом вызове (плюс TOML и CSV).
pub fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(|| {
        let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
        for (grammar, fallback_name) in [
            (TOML_SYNTAX, "TOML"),
            (CSV_SYNTAX, "CSV"),
        ] {
            match SyntaxDefinition::load_from_str(grammar, true, Some(fallback_name)) {
                Ok(def) => builder.add(def),
                // Грамматики в исходнике — ошибки невозможны; защита от
                // регресса — тесты toml_syntax_loads / csv-тесты.
                Err(e) => eprintln!("[syntax] built-in {fallback_name} grammar failed: {e}"),
            }
        }
        builder.build()
    })
}

/// Набор синтексов БЕЗ инициализации: `None`, пока не прогрет.
/// Открытие файла ищет язык только если набор уже готов — бюджет
/// открытия (D11.2) не платит за инициализацию.
pub fn try_syntax_set() -> Option<&'static SyntaxSet> {
    SYNTAX_SET.get()
}

/// Синтаксис по расширению (регистронезависимо).
pub fn detect_by_extension(ext: &str) -> Option<&'static SyntaxReference> {
    let ext = ext.to_ascii_lowercase();
    // TypeScript-семейство → JS-грамматика (набор не содержит TS).
    if TYPESCRIPT_EXTS.contains(&ext.as_str()) {
        return syntax_set().find_syntax_by_extension("js");
    }
    syntax_set().find_syntax_by_extension(&ext)
}

/// Синтаксис по первой строке (shebang `#!/bin/sh`, `<?php`, `<html>` …).
pub fn detect_by_first_line(first_line: &str) -> Option<&'static SyntaxReference> {
    syntax_set().find_syntax_by_first_line(first_line)
}

/// Синтаксис по имени (`SyntaxReference::name`) — ручной выбор.
pub fn syntax_by_name(name: &str) -> Option<&'static SyntaxReference> {
    syntax_set().find_syntax_by_name(name)
}

/// «Plain Text» из набора (ручной сброс подсветки).
pub fn plain_text() -> &'static SyntaxReference {
    syntax_set().find_syntax_plain_text()
}

/// Friendly-имя для статус-бара/списка выбора: длинные синтакс-имена
/// shell → «Shell», прочие — как есть.
pub fn friendly_name(name: &'static str) -> &'static str {
    match name {
        "Bourne Again Shell (bash)" | "Shell-Unix-Generic" => "Shell",
        other => other,
    }
}

/// Определить язык: расширение → эвристика по содержимому (первая
/// строка). Возвращает грамматику + метку для отображения (TypeScript
/// красится JS-грамматикой, метка честная — D12.3). Вызывает
/// `syntax_set()` (инициализация при первом вызове — только с пути
/// прогрева/тестов, не открытия).
pub fn detect_labeled(
    path: Option<&Path>,
    first_line: Option<&str>,
) -> Option<(&'static SyntaxReference, &'static str)> {
    let ext = path
        .and_then(|p| p.extension())
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    if let Some(s) = ext.as_deref().and_then(detect_by_extension) {
        let label = if TYPESCRIPT_EXTS.contains(&ext.as_deref().unwrap_or_default()) {
            "TypeScript"
        } else {
            friendly_name(&s.name)
        };
        return Some((s, label));
    }
    // Без расширения — содержимое: shebang и маркеры первой строки
    // ловит find_syntax_by_first_line. Файлы с неизвестным расширением
    // не эвристим (предсказуемость: .weird с shebang — экзотика).
    if ext.is_none()
        && let Some(s) = first_line.and_then(detect_by_first_line) {
            return Some((s, friendly_name(&s.name)));
        }
    None
}

/// Определить язык без метки (совместимость; грамматика только).
pub fn detect(
    path: Option<&Path>,
    first_line: Option<&str>,
) -> Option<&'static SyntaxReference> {
    detect_labeled(path, first_line).map(|(s, _)| s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Обязательный список спеки (5.4): каждый язык находится по
    /// своему расширению (TypeScript — псевдонимом, D12.3).
    #[test]
    fn mandatory_languages_found_by_extension() {
        let cases = [
            ("c", "C"),
            ("cpp", "C++"),
            ("cs", "C#"),
            ("go", "Go"),
            ("java", "Java"),
            ("js", "JavaScript"),
            ("ts", "JavaScript"), // грамматика (метка — TypeScript)
            ("py", "Python"),
            ("rs", "Rust"),
            ("sql", "SQL"),
            ("html", "HTML"),
            ("xml", "XML"),
            ("json", "JSON"),
            ("yaml", "YAML"),
            ("md", "Markdown"),
            ("sh", "Bourne Again Shell (bash)"),
            ("css", "CSS"),
        ];
        for (ext, name) in cases {
            let s = detect_by_extension(ext).unwrap_or_else(|| panic!("{ext}"));
            assert_eq!(s.name, name, "{ext}");
        }
    }

    /// ≥70 языков: счётчик синтаксисов набора (без Plain Text) — каждый
    /// доступен и авто-детектом (расширение/первая строка), и ручным
    /// выбором. С добавленным TOML: 75.
    #[test]
    fn syntax_set_covers_seventy_plus_languages() {
        let set = syntax_set();
        let count = set
            .syntaxes()
            .iter()
            .filter(|s| s.name != "Plain Text")
            .count();
        assert!(count >= 70, "syntaxes: {count}");
        // Встроенная грамматика TOML загрузилась и ищется.
        assert_eq!(detect_by_extension("toml").unwrap().name, "TOML");
    }

    /// Расширение регистронезависимо.
    #[test]
    fn extension_detection_case_insensitive() {
        assert_eq!(detect_by_extension("RS").unwrap().name, "Rust");
        assert_eq!(detect_by_extension("Py").unwrap().name, "Python");
    }

    /// Эвристика по первой строке: shebang → bash, маркер PHP.
    #[test]
    fn first_line_heuristics() {
        let sh = detect_by_first_line("#!/bin/sh\n");
        assert_eq!(sh.unwrap().name, "Bourne Again Shell (bash)");
        assert!(detect_by_first_line("#!/usr/bin/env python3\n").is_some());
        assert!(detect_by_first_line("<?php echo 1;\n").is_some());
        assert!(detect_by_first_line("just some text\n").is_none());
    }

    /// Полный детект с метками: расширение → TypeScript-метка →
    /// first-line → None.
    #[test]
    fn detect_labeled_extension_first_line() {
        let p = std::path::Path::new("script.py");
        assert_eq!(detect_labeled(Some(p), None).unwrap().1, "Python");
        // TypeScript: грамматика JS, метка TypeScript (D12.3).
        let ts = std::path::Path::new("app.ts");
        let (syntax, label) = detect_labeled(Some(ts), None).unwrap();
        assert_eq!(label, "TypeScript");
        assert_eq!(syntax.name, "JavaScript");
        // Без расширения: shebang решает; friendly-метка.
        let noext = std::path::Path::new("script");
        assert_eq!(
            detect_labeled(Some(noext), Some("#!/bin/bash\n")).unwrap().1,
            "Shell"
        );
        // Неизвестное расширение — эвристику НЕ применяем.
        let weird = std::path::Path::new("file.weird");
        assert!(detect_labeled(Some(weird), Some("#!/bin/sh\n")).is_none());
        assert!(detect_labeled(Some(std::path::Path::new("Makefile")), Some("all:")).is_none());
        assert!(detect_labeled(None, None).is_none());
    }

    /// Поиск по имени (ручной выбор) + Plain Text.
    #[test]
    fn by_name_and_plain_text() {
        assert_eq!(syntax_by_name("YAML").unwrap().name, "YAML");
        assert_eq!(syntax_by_name("TOML").unwrap().name, "TOML");
        assert!(syntax_by_name("No Such Language").is_none());
        assert_eq!(plain_text().name, "Plain Text");
    }

    /// Встроенная TOML-грамматика: валидна и красит базовые элементы
    /// (комментарий/ключ/строка/число/таблица) — через движок подсветки.
    #[test]
    fn toml_syntax_loads_and_highlights() {
        let mut e = crate::highlight::HighlightEngine::new();
        e.set_syntax(Some(detect_by_extension("toml").unwrap()));
        let d = crate::document::Document::from_text(
            "# comment\n[table]\nkey = \"value\"\nn = 42\nok = true\n",
        );
        e.ensure_window(&d, 0, 5);
        let spans = |l: usize| {
            e.spans(l)
                .unwrap()
                .iter()
                .map(|s| (s.start, s.end, s.class))
                .collect::<Vec<_>>()
        };
        use crate::highlight::*;
        assert_eq!(spans(0), vec![(0, 9, CLASS_COMMENT)]);
        assert!(spans(1).iter().any(|(s, t, c)| *c == CLASS_TYPE && *t > *s), "{:?}",
            spans(1));
        assert!(spans(2).iter().any(|(_, _, c)| *c == CLASS_TYPE), "{:?}", spans(2)); // ключ
        assert!(spans(2).iter().any(|(_, _, c)| *c == CLASS_STRING), "{:?}", spans(2));
        assert!(spans(3).iter().any(|(_, _, c)| *c == CLASS_NUMBER), "{:?}", spans(3));
        assert!(spans(4).iter().any(|(_, _, c)| *c == CLASS_CONSTANT), "{:?}", spans(4));
    }
}
