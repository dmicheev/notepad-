//! Язык файла для статус-бара (задача 3.5) — минимальный маппинг по
//! расширению; полное определение (эвристика по содержимому,
//! синтаксисы syntect, ручной выбор) — задача 5.1.

use std::path::Path;

/// Определить язык по расширению пути; untitled/без расширения/
/// неизвестное расширение → "Plain Text" (D8.5).
pub fn language_for_path(path: Option<&Path>) -> &'static str {
    let ext = path
        .and_then(|p| p.extension())
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "rs" => "Rust",
        "py" | "pyi" => "Python",
        "js" | "mjs" | "cjs" => "JavaScript",
        "ts" | "tsx" => "TypeScript",
        "jsx" => "JavaScript JSX",
        "go" => "Go",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "swift" => "Swift",
        "c" | "h" => "C",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => "C++",
        "cs" => "C#",
        "rb" => "Ruby",
        "php" => "PHP",
        "sh" | "bash" => "Shell",
        "zsh" => "Zsh",
        "fish" => "Fish",
        "ps1" => "PowerShell",
        "html" | "htm" => "HTML",
        "css" => "CSS",
        "scss" | "sass" => "Sass",
        "less" => "Less",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "xml" => "XML",
        "md" | "markdown" => "Markdown",
        "sql" => "SQL",
        "csv" | "tsv" => "CSV",
        "ini" | "cfg" | "conf" => "INI",
        "bat" | "cmd" => "Batch",
        "dockerfile" => "Dockerfile",
        "lua" => "Lua",
        "pl" => "Perl",
        "r" => "R",
        "scala" => "Scala",
        "hs" => "Haskell",
        "vb" => "Visual Basic",
        "asm" | "s" => "Assembly",
        "txt" | "log" => "Plain Text",
        _ => "Plain Text",
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
}
