//! Настройки воркспейса: скрытая папка `.settings` в корне открытого
//! воркспейса — персистентное состояние редактора, привязанное к
//! файлам (закладки/фолды). Формат расширяем (`version`): будущие
//! настройки воркспейса добавляются секциями того же TOML-файла.
//!
//! Файл: `.settings/workspace.toml`
//! ```toml
//! version = 1
//!
//! [files."notes/idea.txt"]
//! bookmarks = [2, 7]
//! folds = [10]
//!
//! [files."data/table.csv"]
//! csv_delimiter = ";"
//! csv_quote = "\""
//! ```
//!
//! Дерево воркспейса скрытые «.»-записи не показывает (fs_tree) —
//! папка невидима в UI. Файлы вне корня (и работа без воркспейса)
//! настроек не имеют — их метки живут только в памяти вкладки.
//! Записи закрытых файлов сохраняются: закладки переживают закрытие
//! и возвращаются при повторном открытии (merge при записи).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::workspace::Workspace;

/// Имя скрытой папки настроек в корне воркспейса.
pub(crate) const SETTINGS_DIR: &str = ".settings";
/// Имя файла настроек (TOML, человекочитаем/ручно-правим).
pub(crate) const SETTINGS_FILE: &str = "workspace.toml";
/// Текущая версия формата (миграции — по мере надобности).
pub(crate) const VERSION: u32 = 1;

/// Состояние одного файла: строковые метки редактора.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileState {
    /// Строки с закладками (0-based, как LineMeta).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bookmarks: Vec<usize>,
    /// Свёрнутые строки-заголовки фолдов.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folds: Vec<usize>,
    /// Разделитель CSV (D12.6): одиночный символ или слово «auto»
    /// (авто-детект по строке). Отсутствие — явного выбора не было
    /// (при открытии CSV спросит снова).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csv_delimiter: Option<String>,
    /// Квотирование CSV: одиночный символ или слово «none» (без
    /// квотирования). Отсутствие — явного выбора не было.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csv_quote: Option<String>,
}

impl FileState {
    pub fn is_empty(&self) -> bool {
        self.bookmarks.is_empty()
            && self.folds.is_empty()
            && self.csv_delimiter.is_none()
            && self.csv_quote.is_none()
    }

    /// Явный выбор CSV-настроек (D12.6); `None` — выбора не было
    /// (поля отсутствуют/битые — не ломают открытие).
    pub fn csv_options(&self) -> Option<editor_core::CsvOptions> {
        let delimiter = self.csv_delimiter.as_deref().and_then(|s| parse_choice(s, "auto"))?;
        let quote = self.csv_quote.as_deref().and_then(|s| parse_choice(s, "none"))?;
        Some(editor_core::CsvOptions { delimiter, quote })
    }

    /// Поля TOML из явного выбора (обратное преобразование
    /// `csv_options`; пишет всегда оба поля).
    pub fn csv_fields(options: editor_core::CsvOptions) -> (Option<String>, Option<String>) {
        let d = options.delimiter.map(|c| c.to_string()).unwrap_or_else(|| "auto".into());
        let q = options.quote.map(|c| c.to_string()).unwrap_or_else(|| "none".into());
        (Some(d), Some(q))
    }
}

/// Значение поля CSV-выбора: служебное слово (`auto`/`none` → None)
/// или РОВНО один символ; прочее — None (битая запись = выбора нет).
fn parse_choice(s: &str, none_word: &str) -> Option<Option<char>> {
    if s == none_word {
        return Some(None);
    }
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(Some(c)),
        _ => None,
    }
}

/// Настройки воркспейса целиком; ключ `files` — относительный путь
/// от корня воркспейса (разделители — `/`, платформонезависимо).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct WorkspaceSettings {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub files: BTreeMap<String, FileState>,
}

/// Путь файла настроек: `<root>/.settings/workspace.toml`.
pub(crate) fn settings_path(root: &Path) -> PathBuf {
    root.join(SETTINGS_DIR).join(SETTINGS_FILE)
}

/// Загрузить настройки; нет файла/битый TOML — дефолт (настройки не
/// должны ломать открытие воркспейса, ошибка — в stderr).
pub(crate) fn load(root: &Path) -> WorkspaceSettings {
    let read = match fs::read_to_string(settings_path(root)) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return WorkspaceSettings::default(),
        Err(e) => {
            eprintln!("[settings] read failed: {e}");
            return WorkspaceSettings::default();
        }
    };
    match toml::from_str(&read) {
        Ok(settings) => settings,
        Err(e) => {
            eprintln!("[settings] parse failed: {e}");
            WorkspaceSettings::default()
        }
    }
}

/// Сохранить настройки (создаёт `.settings` при необходимости).
pub(crate) fn save(root: &Path, settings: &WorkspaceSettings) -> io::Result<()> {
    let path = settings_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut to_save = settings.clone();
    to_save.version = VERSION;
    let text = toml::to_string_pretty(&to_save)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(path, text)
}

/// Относительный путь файла внутри корня воркспейса (`None` — файл
/// вне корня); разделители нормализуются в `/`.
pub(crate) fn rel_within(path: &Path, root: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
}

impl Workspace {
    /// Корень открытого воркспейса (None — воркспейс не открыт).
    pub(crate) fn workspace_root(&self) -> Option<&Path> {
        self.tree.as_ref().map(|t| t.root())
    }

    /// Сохранённый выбор CSV-настроек файла (D12.6): `None` — файла
    /// нет в настройках воркспейса либо выбора не было (спросить).
    pub(crate) fn saved_csv_options(&self, path: &Path) -> Option<editor_core::CsvOptions> {
        let root = self.workspace_root()?;
        let rel = rel_within(path, root)?;
        self.workspace_settings.files.get(&rel)?.csv_options()
    }

    /// Загрузить настройки воркспейса (открытие/смена воркспейса) и
    /// сбросить гидратацию: новая вселенная меток — вкладки (в т.ч.
    /// оставшиеся от прежнего воркспейса) применяют метки заново.
    pub(crate) fn load_workspace_settings(&mut self, root: &Path) {
        self.workspace_settings = load(root);
        for pane in &mut self.panes {
            for tab in pane.model.tabs_mut() {
                tab.marks_hydrated = false;
            }
        }
    }

    /// Применить сохранённые метки к вкладкам после открытия: один
    /// раз на ВКЛАДКУ (marks_hydrated — повторное открытие файла это
    /// новая вкладка). Вызывается из render ДО ensure_fold_view —
    /// единая точка после ВСЕХ путей открытия вкладок (дерево, поиск,
    /// сессия, дроп), без хуков в каждом.
    pub(crate) fn hydrate_workspace_marks(&mut self) {
        let Some(root) = self.workspace_root().map(Path::to_path_buf) else {
            return;
        };
        // Проход 1 (иммутабельный): собрать метки негидратированных
        // вкладок под корнем (клон — применяем вне заёма).
        let mut pending: Vec<(usize, usize, FileState)> = Vec::new();
        for (pi, pane) in self.panes.iter().enumerate() {
            for (ti, tab) in pane.model.tabs().iter().enumerate() {
                if tab.marks_hydrated {
                    continue;
                }
                let Some(path) = &tab.path else { continue };
                let Some(rel) = rel_within(path, &root) else { continue };
                if let Some(state) = self.workspace_settings.files.get(&rel) {
                    pending.push((pi, ti, state.clone()));
                }
            }
        }
        // Проход 2: применить; пометка «гидратирована» — ВСЕМ вкладкам
        // под корнем (в т.ч. без записи в настройках), иначе беззаписные
        // файлы rescanned каждый кадр.
        for pane in &mut self.panes {
            for tab in pane.model.tabs_mut() {
                if let Some(path) = &tab.path
                    && rel_within(path, &root).is_some()
                {
                    tab.marks_hydrated = true;
                }
            }
        }
        for (pi, ti, state) in pending {
            if let Some(tab) = self.panes.get_mut(pi).and_then(|p| p.model.tab_mut(ti)) {
                tab.apply_line_marks(&state.bookmarks, &state.folds);
                // Сохранённый выбор CSV (D12.6): применяется молча,
                // вопрос при открытии не нужен.
                if let Some(options) = state.csv_options() {
                    tab.set_csv_options(options);
                    tab.csv_asked = true;
                }
            }
        }
    }

    /// Записать метки открытых вкладок в `.settings` (merge: записи
    /// файлов, чьи вкладки уже закрыты, сохраняются — закладки
    /// переживают закрытие и возвращаются при повторном открытии).
    /// Вызывается на toggle закладки/фолда, закрытие вкладки и выход.
    pub(crate) fn persist_workspace_marks(&mut self) {
        let Some(root) = self.workspace_root().map(Path::to_path_buf) else {
            return;
        };
        let mut settings = self.workspace_settings.clone();
        let mut updated: Vec<(String, FileState)> = Vec::new();
        for pane in &self.panes {
            for tab in pane.model.tabs() {
                let Some(path) = &tab.path else { continue };
                let Some(rel) = rel_within(path, &root) else { continue };
                // Явный выбор CSV (D12.6) — только для CSV-вкладок, где
                // выбор уже сделан (диалог показан/сохранён ранее).
                let (csv_delimiter, csv_quote) =
                    if tab.is_csv() && tab.csv_asked {
                        FileState::csv_fields(tab.csv_options)
                    } else {
                        (None, None)
                    };
                updated.push((
                    rel,
                    FileState {
                        bookmarks: tab.doc.line_meta().bookmarked_lines(),
                        folds: tab.doc.line_meta().folded_lines(),
                        csv_delimiter,
                        csv_quote,
                    },
                ));
            }
        }
        for (rel, state) in updated {
            if state.is_empty() {
                settings.files.remove(&rel);
            } else {
                settings.files.insert(rel, state);
            }
        }
        if let Err(e) = save(&root, &settings) {
            eprintln!("[settings] save failed: {e}");
        }
        self.workspace_settings = settings;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// Round-trip: save пишет TOML с version, load читает обратно.
    #[test]
    fn save_load_roundtrip() {
        let dir = TempDir::new("ws-settings-rt");
        let mut settings = WorkspaceSettings::default();
        settings.files.insert(
            "notes/idea.txt".into(),
            FileState { bookmarks: vec![2, 7], folds: vec![10], ..Default::default() },
        );
        settings.files.insert("todo.txt".into(), FileState { bookmarks: vec![0], ..Default::default() });
        save(dir.path(), &settings).unwrap();

        let file = settings_path(dir.path());
        assert!(file.starts_with(dir.path().join(SETTINGS_DIR)));
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("version = 1"), "версия в файле: {text}");
        assert!(text.contains("[files.\"notes/idea.txt\"]"), "секция файла: {text}");

        let back = load(dir.path());
        assert_eq!(back.version, VERSION);
        assert_eq!(back.files.len(), 2);
        assert_eq!(
            back.files.get("notes/idea.txt"),
            Some(&FileState { bookmarks: vec![2, 7], folds: vec![10], ..Default::default() })
        );
        // Пустые списки не сериализуются (skip_serializing_if).
        assert_eq!(back.files.get("todo.txt"), Some(&FileState { bookmarks: vec![0], ..Default::default() }));
    }

    /// Нет файла — дефолт; битый TOML — дефолт (не паника).
    #[test]
    fn load_missing_and_malformed() {
        let dir = TempDir::new("ws-settings-bad");
        assert_eq!(load(dir.path()), WorkspaceSettings::default());
        fs::create_dir_all(dir.path().join(SETTINGS_DIR)).unwrap();
        fs::write(settings_path(dir.path()), "not [ toml").unwrap();
        assert_eq!(load(dir.path()), WorkspaceSettings::default());
    }

    /// Частичный TOML (старые версии) десериализуется с дефолтами.
    #[test]
    fn partial_toml_defaults() {
        let dir = TempDir::new("ws-settings-partial");
        fs::create_dir_all(dir.path().join(SETTINGS_DIR)).unwrap();
        fs::write(
            settings_path(dir.path()),
            "[files.\"a.txt\"]\nbookmarks = [1]\n",
        )
        .unwrap();
        let settings = load(dir.path());
        assert_eq!(settings.version, 0, "версии нет — дефолт");
        assert_eq!(
            settings.files.get("a.txt"),
            Some(&FileState { bookmarks: vec![1], ..Default::default() })
        );
    }

    /// rel_within: внутри корня — нормализованный путь, вне — None.
    #[test]
    fn rel_within_bounds() {
        let root = Path::new("/ws/root");
        assert_eq!(rel_within(&Path::new("/ws/root/a/b.md"), root), Some("a/b.md".into()));
        assert_eq!(rel_within(&Path::new("/ws/root/notes.md"), root), Some("notes.md".into()));
        assert_eq!(rel_within(&Path::new("/elsewhere/x.md"), root), None);
    }

    /// CSV-выбор (D12.6): round-trip FileState ↔ CsvOptions; служебные
    /// слова auto/none; битые значения = выбора не было; отсутствие
    /// полей = вопроса не было.
    #[test]
    fn csv_state_roundtrip_and_parse() {
        // Полный выбор: `;` + без квотирования.
        let state = FileState {
            csv_delimiter: Some(";".into()),
            csv_quote: Some("none".into()),
            ..Default::default()
        };
        assert_eq!(
            state.csv_options(),
            Some(editor_core::CsvOptions { delimiter: Some(';'), quote: None })
        );
        // Авто-разделитель + одинарная кавычка.
        let state = FileState {
            csv_delimiter: Some("auto".into()),
            csv_quote: Some("'".into()),
            ..Default::default()
        };
        assert_eq!(
            state.csv_options(),
            Some(editor_core::CsvOptions { delimiter: None, quote: Some('\'') })
        );
        // Обратное преобразование.
        let (d, q) = FileState::csv_fields(editor_core::CsvOptions {
            delimiter: Some('\t'),
            quote: None,
        });
        assert_eq!((d.as_deref(), q.as_deref()), (Some("\t"), Some("none")));
        let (d, q) =
            FileState::csv_fields(editor_core::CsvOptions::default());
        assert_eq!((d.as_deref(), q.as_deref()), (Some("auto"), Some("\"")));
        // Битые записи — считаем «выбора не было» (не ломают открытие).
        for bad in ["", "ab", "\"\""] {
            let state = FileState {
                csv_delimiter: Some(bad.into()),
                ..Default::default()
            };
            assert_eq!(state.csv_options(), None, "битый разделитель {bad:?}");
        }
        // Поля нет — вопроса не было.
        assert_eq!(FileState::default().csv_options(), None);
        // Не-пустое состояние с одним только CSV-выбором.
        assert!(!FileState {
            csv_delimiter: Some(",".into()),
            ..Default::default()
        }
        .is_empty());
        // TOML round-trip: выбор переживает save/load.
        let dir = TempDir::new("ws-settings-csv");
        let mut settings = WorkspaceSettings::default();
        settings.files.insert(
            "data.csv".into(),
            FileState {
                csv_delimiter: Some(";".into()),
                csv_quote: Some("none".into()),
                ..Default::default()
            },
        );
        save(dir.path(), &settings).unwrap();
        let text = fs::read_to_string(settings_path(dir.path())).unwrap();
        assert!(text.contains("csv_delimiter = \";\""), "{text}");
        assert!(text.contains("csv_quote = \"none\""), "{text}");
        let back = load(dir.path());
        assert_eq!(
            back.files.get("data.csv").and_then(|s| s.csv_options()),
            Some(editor_core::CsvOptions { delimiter: Some(';'), quote: None })
        );
    }
}
