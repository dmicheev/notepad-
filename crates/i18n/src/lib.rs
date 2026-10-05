//! i18n (8.4/D15.4): строки UI во внешних файлах переводов (TOML,
//! один файл на язык; `locales/en.toml` — источник, вшит в бинарник
//! как fallback). Каталог внешних переводов —
//! `<data dir>/notepad-plus/locales/`; новый язык добавляется
//! файлом `<lang>.toml` + выбором в настройках, БЕЗ пересборки.
//!
//! Ключи — плоские точки (`menu.file.open`); TOML-таблицы
//! разворачиваются рекурсивно. Поиск: выбранный язык → en (внешний
//! → вшитый) → ключ (разработчик увидел сырой ключ).

use std::collections::HashMap;
use std::path::Path;

/// Вшитый источник английских строк (fallback любого языка).
const EMBEDDED_EN: &str = include_str!("../locales/en.toml");

/// Состояние переводов: выбранный язык поверх английского.
#[derive(Debug, Clone)]
pub struct I18n {
    language: String,
    /// Строки выбранного языка (может быть неполным).
    selected: HashMap<String, String>,
    /// Английский (внешний поверх вшитого).
    en: HashMap<String, String>,
}

impl I18n {
    /// Строка по ключу: выбранный язык → en → ключ.
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        if let Some(s) = self.selected.get(key) {
            return s.as_str();
        }
        if let Some(s) = self.en.get(key) {
            return s.as_str();
        }
        key
    }

    /// Строка с подстановками: плейсхолдеры `{name}` заменяются
    /// значениями (порядок произвольный, отсутствующие остаются).
    pub fn tf(&self, key: &str, args: &[(&str, &str)]) -> String {
        let mut out = self.t(key).to_string();
        for (name, value) in args {
            out = out.replace(&format!("{{{name}}}"), value);
        }
        out
    }

    /// Выбранный язык (код файла: "en", "ru", …).
    pub fn language(&self) -> &str {
        &self.language
    }

    /// Загрузить переводы: вшитый en ← внешний en (если есть) ←
    /// внешний файл выбранного языка (если есть). Отсутствие файлов —
    /// не ошибка: работает вшитый английский.
    pub fn load(dir: Option<&Path>, language: &str) -> Self {
        let mut en = flatten_toml(EMBEDDED_EN);
        if let Some(dir) = dir
            && let Ok(ext) = std::fs::read_to_string(dir.join("en.toml"))
        {
            en.extend(flatten_toml(&ext));
        }
        let mut selected = HashMap::new();
        if language != "en"
            && let Some(dir) = dir
            && let Ok(file) = std::fs::read_to_string(dir.join(format!("{language}.toml")))
        {
            selected = flatten_toml(&file);
        }
        I18n {
            language: language.to_string(),
            selected,
            en,
        }
    }

    /// Доступные языки: "en" + стемы `*.toml` внешнего каталога
    /// (отсортировано, без дублей).
    pub fn available(dir: Option<&Path>) -> Vec<String> {
        let mut out = vec!["en".to_string()];
        if let Some(dir) = dir
            && let Ok(entries) = std::fs::read_dir(dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "toml")
                    && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                    && !out.iter().any(|l| l == stem) {
                        out.push(stem.to_string());
                    }
            }
        }
        out.sort();
        out
    }
}

/// Разворачивает TOML-таблиццы в плоские ключи ("[menu.file]\nopen =
/// …" → "menu.file.open"). Скаляры — строки; не-строки — serde_json
/// (допустимо для простых значений).
fn flatten_toml(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(table) = text.parse::<toml::Table>() else {
        return out;
    };
    flatten_table(&table, "", &mut out);
    out
}

fn flatten_table(table: &toml::Table, prefix: &str, out: &mut HashMap<String, String>) {
    for (key, value) in table {
        let full = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            toml::Value::Table(sub) => flatten_table(sub, &full, out),
            toml::Value::String(s) => {
                out.insert(full, s.clone());
            }
            other => {
                out.insert(full, other.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("notepad-i18n-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Вшитый en.toml парсится и содержит ключи меню.
    #[test]
    fn embedded_en_parses() {
        let i18n = I18n::load(None, "en");
        assert_eq!(i18n.t("menu.file.open"), "Open…");
        assert_eq!(i18n.language(), "en");
    }

    /// Fallback: отсутствующий ключ в выбранном языке → английский.
    #[test]
    fn fallback_to_english() {
        let dir = tmpdir("fallback");
        std::fs::write(
            dir.join("xx.toml"),
            "[menu.file]\nopen = \"Xx-Open\"\n",
        )
        .unwrap();
        let i18n = I18n::load(Some(&dir), "xx");
        assert_eq!(i18n.t("menu.file.open"), "Xx-Open", "переведённый ключ");
        assert_ne!(i18n.t("menu.file.save"), "Xx-Open");
        assert!(!i18n.t("menu.file.save").is_empty(), "остальное — en");
    }

    /// Спека internationalization: подмена файла перевода меняет язык
    /// БЕЗ пересборки — внешний en.toml перекрывает вшитый.
    #[test]
    fn external_en_overrides_embedded() {
        let dir = tmpdir("override");
        std::fs::write(
            dir.join("en.toml"),
            "[menu.file]\nopen = \"Open (custom)\"\n",
        )
        .unwrap();
        let i18n = I18n::load(Some(&dir), "en");
        assert_eq!(i18n.t("menu.file.open"), "Open (custom)");
        // Остальные ключи остались вшитыми.
        assert_eq!(i18n.t("menu.file.save"), "Save");
    }

    /// Отсутствующий каталог/язык — вшитый английский, без паник.
    #[test]
    fn missing_files_fall_back_to_embedded() {
        let dir = tmpdir("missing");
        let i18n = I18n::load(Some(&dir), "ru");
        assert_eq!(i18n.t("menu.file.open"), "Open…");
        assert_eq!(i18n.t("nonexistent.key"), "nonexistent.key", "сырой ключ");
    }

    /// available: en + стемы внешних toml.
    #[test]
    fn available_lists_external_languages() {
        let dir = tmpdir("available");
        std::fs::write(dir.join("ru.toml"), "[x]\ny = \"z\"\n").unwrap();
        std::fs::write(dir.join("de.toml"), "[x]\ny = \"z\"\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        let langs = I18n::available(Some(&dir));
        assert_eq!(langs, vec!["de".to_string(), "en".to_string(), "ru".to_string()]);
        assert_eq!(I18n::available(None), vec!["en".to_string()]);
    }

    /// tf: подстановка плейсхолдеров.
    #[test]
    fn formats_placeholders() {
        let dir = tmpdir("fmt");
        std::fs::write(
            dir.join("xx.toml"),
            "[status]\ncursors = \"{n} курсоров\"\n",
        )
        .unwrap();
        let i18n = I18n::load(Some(&dir), "xx");
        assert_eq!(i18n.tf("status.cursors", &[("n", "3")]), "3 курсоров");
        // en-шаблон с параметрами.
        assert_eq!(i18n.tf("menu.app.quit", &[]), "Quit");
    }
}
