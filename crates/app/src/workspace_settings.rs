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
}

impl FileState {
    pub fn is_empty(&self) -> bool {
        self.bookmarks.is_empty() && self.folds.is_empty()
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
                updated.push((
                    rel,
                    FileState {
                        bookmarks: tab.doc.line_meta().bookmarked_lines(),
                        folds: tab.doc.line_meta().folded_lines(),
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
            FileState { bookmarks: vec![2, 7], folds: vec![10] },
        );
        settings.files.insert("todo.txt".into(), FileState { bookmarks: vec![0], folds: vec![] });
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
            Some(&FileState { bookmarks: vec![2, 7], folds: vec![10] })
        );
        // Пустые списки не сериализуются (skip_serializing_if).
        assert_eq!(back.files.get("todo.txt"), Some(&FileState { bookmarks: vec![0], folds: vec![] }));
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
            Some(&FileState { bookmarks: vec![1], folds: vec![] })
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
}
