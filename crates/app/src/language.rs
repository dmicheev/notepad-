//! Язык файла (задача 5.1; до — маппинг 3.5/D8.5) — обёртка над
//! syntect-детектом editor-core (syntax.rs): грамматика + friendly-метка
//! для статус-бара. Полное определение (расширение → содержимое),
//! ручной выбор — оверлей language_dialog.rs; подсветка — highlight.rs.

use std::path::Path;

use editor_core::SyntaxReference;

use editor_core::syntax::{detect_labeled, friendly_name, syntax_by_name, try_syntax_set};

/// Порог отключения подсветки (спека syntax-highlighting): файл больше
/// 20 МБ (байтов) открывается редактируемым, но без подсветки; язык в
/// статус-баре «Plain Text», ручной выбор запрещён (D12.3).
pub const HIGHLIGHT_MAX_BYTES: u64 = 20 * 1024 * 1024;

/// Грамматика + метка языка (метка ≠ имя грамматики для
/// TypeScript-псевдонима — D12.3).
pub type LabeledSyntax = (&'static SyntaxReference, &'static str);

/// Определить язык файла: расширение → первая строка документа
/// (эвристика shebang/маркеров). Инициализирует набор синтаксисов при
/// первом вызове — только с пути прогрева/тестов; открытие файла
/// использует `detect_if_warm`.
pub fn detect_language(path: Option<&Path>, first_line: Option<&str>) -> Option<LabeledSyntax> {
    detect_labeled(path, first_line)
}

/// Определить язык ТОЛЬКО если набор синтаксисов уже прогрет (прогрев
/// после первого кадра — D12.1); иначе `None` (до прогрева язык
/// показывается как Plain Text).
pub fn detect_if_warm(path: Option<&Path>, first_line: Option<&str>) -> Option<LabeledSyntax> {
    try_syntax_set()?;
    detect_language(path, first_line)
}

/// Язык по пути для статус-бара (без содержимого: расширение только);
/// тесты 3.5. В рендере статус-бара НЕ используется — там состояние
/// вкладки (BufferTab::language_label).
#[allow(dead_code)] // тестовая поверхность 3.5 (статус-бар читает состояние вкладки)
pub fn language_for_path(path: Option<&Path>) -> &'static str {
    detect_language(path, None)
        .map(|(_, label)| label)
        .unwrap_or("Plain Text")
}

/// Список языков для ручного выбора (оверлей 5.1): синтаксисы набора с
/// расширениями (без Plain Text — отдельная запись), отсортированные
/// по имени; TypeScript-псевдоним добавляем меткой отдельной строкой.
pub fn language_list() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = editor_core::syntax::syntax_set()
        .syntaxes()
        .iter()
        .filter(|s| !s.file_extensions.is_empty() && s.name != "Plain Text")
        .map(|s| friendly_name(&s.name))
        .collect();
    names.push("Shell"); // метка выбирает Bourne Again Shell (bash)
    names.push("TypeScript"); // псевдоним JS-грамматики (D12.3)
    names.sort_unstable();
    names.dedup();
    names
}

/// Разрешить метку списка в грамматику (выбор пользователя).
/// Возвращает None для «Plain Text» (подсветка выключена).
pub fn syntax_for_choice(choice: &str) -> Option<&'static SyntaxReference> {
    match choice {
        "Plain Text" => None,
        "TypeScript" => syntax_by_name("JavaScript"), // псевдоним
        "Shell" => syntax_by_name("Bourne Again Shell (bash)"),
        other => syntax_by_name(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn common_extensions() {
        let cases = [
            ("main.rs", "Rust"),
            ("script.py", "Python"),
            ("app.ts", "TypeScript"),
            ("index.js", "JavaScript"),
            ("main.go", "Go"),
            ("Main.java", "Java"),
            ("lib.cpp", "C++"),
            ("styles.css", "CSS"),
            ("data.json", "JSON"),
            ("conf.yaml", "YAML"),
            ("Cargo.toml", "TOML"),
            ("README.md", "Markdown"),
            ("query.sql", "SQL"),
            ("run.sh", "Shell"),
            ("index.html", "HTML"),
            ("notes.txt", "Plain Text"),
        ];
        for (name, lang) in cases {
            assert_eq!(language_for_path(Some(&PathBuf::from(name))), lang, "{name}");
        }
    }

    /// Расширение регистронезависимо.
    #[test]
    fn extension_case_insensitive() {
        assert_eq!(language_for_path(Some(&PathBuf::from("MAIN.RS"))), "Rust");
        assert_eq!(language_for_path(Some(&PathBuf::from("readme.MD"))), "Markdown");
    }

    /// Untitled, без расширения, неизвестное — Plain Text.
    #[test]
    fn fallbacks_are_plain_text() {
        assert_eq!(language_for_path(None), "Plain Text");
        assert_eq!(language_for_path(Some(&PathBuf::from("Makefile"))), "Plain Text");
        assert_eq!(language_for_path(Some(&PathBuf::from("file.xyz"))), "Plain Text");
    }

    /// Детект по содержимому: shebang без расширения → Shell.
    #[test]
    fn first_line_detection() {
        let (syntax, label) =
            detect_language(Some(&PathBuf::from("script")), Some("#!/bin/sh\n")).unwrap();
        assert_eq!(label, "Shell");
        assert_eq!(syntax.name, "Bourne Again Shell (bash)");
    }

    /// Список ручного выбора: обязательные языки спеки присутствуют
    /// (TypeScript — псевдонимом). Требование «≥70 языков» держит
    /// НАБОР синтаксисов (editor-core, 75 ≥ 70); пикер показывает
    /// подмножество с расширениями (~64) — без служебных синтаксисов
    /// без расширений (Make Output, JavaDoc, …).
    #[test]
    fn language_list_covers_mandatory() {
        let list = language_list();
        assert!(list.len() >= 60, "list: {}", list.len());
        for lang in [
            "C", "C++", "C#", "Go", "Java", "JavaScript", "TypeScript", "Python", "Rust", "SQL",
            "HTML", "XML", "JSON", "YAML", "TOML", "Markdown", "Shell", "CSS",
        ] {
            assert!(list.contains(&lang), "{lang} missing");
        }
    }

    /// Выбор из списка разрешается в грамматику; Plain Text → None.
    #[test]
    fn choice_resolves_to_syntax() {
        assert_eq!(syntax_for_choice("Rust").unwrap().name, "Rust");
        assert_eq!(syntax_for_choice("TypeScript").unwrap().name, "JavaScript");
        assert_eq!(
            syntax_for_choice("Shell").unwrap().name,
            "Bourne Again Shell (bash)"
        );
        assert!(syntax_for_choice("Plain Text").is_none());
        assert!(syntax_for_choice("No Such").is_none());
    }

    /// Порог подсветки — константа спеки (20 МБ).
    #[test]
    fn threshold_is_twenty_mebibytes() {
        assert_eq!(HIGHLIGHT_MAX_BYTES, 20 * 1024 * 1024);
    }
}
