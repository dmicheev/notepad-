//! Состояние сессии (задача 3.4) — GPUI-free модель: JSON (serde) в
//! каталоге данных ОС. Поля: воркспейс (пока None — группа 7), список
//! вкладок с путями, активная вкладка, геометрия окна.
//!
//! Untitled-вкладки не сохраняются (контент живёт только в памяти),
//! dirty-флаг не сохраняется — восстановленные вкладки перечитываются
//! с диска; несохранённые изменения при выходе теряются (D8.4).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Имя каталога приложения в каталоге данных ОС.
pub const APP_DIR: &str = "notepad-plus";
/// Имя файла сессии.
pub const SESSION_FILE: &str = "session.json";

/// Состояние сессии.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionState {
    /// Воркспейс (группа 7); пока всегда None.
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    /// Открытые вкладки (только с путями; untitled пропускаются).
    #[serde(default)]
    pub tabs: Vec<SessionTab>,
    /// Индекс активной вкладки в `tabs` на момент сохранения.
    #[serde(default)]
    pub active_tab: Option<usize>,
    /// Геометрия окна.
    #[serde(default)]
    pub window: Option<WindowGeometry>,
}

/// Вкладка сессии: путь файла.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionTab {
    pub path: PathBuf,
}

/// Геометрия окна (windowed bounds, px).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl WindowGeometry {
    /// Минимально осмысленная геометрия (валидация при восстановлении).
    pub fn is_sane(&self) -> bool {
        self.width.is_finite()
            && self.height.is_finite()
            && self.x.is_finite()
            && self.y.is_finite()
            && self.width >= 200.0
            && self.height >= 200.0
    }
}

/// Каталог данных приложения (macOS: ~/Library/Application Support/
/// notepad-plus) — через crate `dirs` (D8.4), без хардкода home.
pub fn app_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(APP_DIR))
}

/// Путь файла сессии.
pub fn session_path() -> Option<PathBuf> {
    app_data_dir().map(|d| d.join(SESSION_FILE))
}

/// Сохранить сессию (создаёт каталог при необходимости).
pub fn save_session(state: &SessionState, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(to, json)
}

/// Загрузить сессию; `Ok(None)` — файла ещё нет (первый запуск).
pub fn load_session(from: &Path) -> io::Result<Option<SessionState>> {
    match fs::read_to_string(from) {
        Ok(json) => {
            let state = serde_json::from_str(&json)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Ok(Some(state))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// План восстановления сессии: вкладки к открытию и пропущенные
/// (файл удалён к моменту рестарта — сообщением в статус-бар, D8.4).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RestorePlan {
    /// Пути к открытию, в исходном порядке.
    pub tabs: Vec<PathBuf>,
    /// Пути, пропущенные из-за отсутствия файла.
    pub skipped: Vec<PathBuf>,
    /// Индекс активной вкладки ВНУТРИ `tabs` (пересчитан).
    pub active: Option<usize>,
}

/// Построить план восстановления: существующие вкладки открываются,
    /// отсутствующие пропускаются; активная вкладка пересчитывается —
/// если она была пропущена, активной становится ближайшая предыдущая
/// выжившая (иначе первая).
pub fn plan_restore(state: &SessionState, exists: impl Fn(&Path) -> bool) -> RestorePlan {
    let mut plan = RestorePlan::default();
    // Старый индекс -> новый индекс для выживших вкладок.
    let mut remap: Vec<Option<usize>> = Vec::with_capacity(state.tabs.len());
    for tab in &state.tabs {
        if exists(&tab.path) {
            remap.push(Some(plan.tabs.len()));
            plan.tabs.push(tab.path.clone());
        } else {
            remap.push(None);
            plan.skipped.push(tab.path.clone());
        }
    }
    plan.active = state
        .active_tab
        .and_then(|a| remap.get(a).copied().flatten())
        .or_else(|| {
            // Активная пропущена: ближайшая предыдущая выжившая, иначе
            // последняя выжившая (перед None-активом в конце).
            remap.iter().flatten().copied().max()
        });
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// Round-trip сериализации полной модели.
    #[test]
    fn session_state_roundtrip() {
        let state = SessionState {
            // Воркспейс (7.4) сериализуется как путь.
            workspace: Some(PathBuf::from("/tmp/notes-root")),
            tabs: vec![
                SessionTab { path: PathBuf::from("/tmp/a.txt") },
                SessionTab { path: PathBuf::from("/tmp/b.rs") },
            ],
            active_tab: Some(1),
            window: Some(WindowGeometry { x: 10.5, y: 20.0, width: 1024.0, height: 768.0 }),
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    /// Дефолтная (пустая) сессия сериализуется/десериализуется; поля
    /// отсутствующие в JSON принимают дефолты (forward-compat).
    #[test]
    fn empty_and_partial_state_deserialize() {
        let back: SessionState = serde_json::from_str("{}").unwrap();
        assert_eq!(back, SessionState::default());

        let partial: SessionState =
            serde_json::from_str(r#"{"tabs": [{"path": "/tmp/x.txt"}]}"#).unwrap();
        assert_eq!(partial.tabs.len(), 1);
        assert_eq!(partial.active_tab, None);
        assert_eq!(partial.window, None);
    }

    /// save_session создаёт каталоги и файл; load читает обратно.
    #[test]
    fn save_and_load_to_disk() {
        let dir = TempDir::new("save-load");
        let target = dir.path().join("nested").join("appdata").join(SESSION_FILE);
        let state = SessionState {
            workspace: None,
            tabs: vec![SessionTab { path: dir.write("note.txt", "content\n") }],
            active_tab: Some(0),
            window: Some(WindowGeometry { x: 0.0, y: 0.0, width: 800.0, height: 600.0 }),
        };
        save_session(&state, &target).unwrap();
        assert!(target.exists());
        let back = load_session(&target).unwrap().unwrap();
        assert_eq!(back, state);
    }

    /// Отсутствующий файл сессии — Ok(None); битый JSON — ошибка.
    #[test]
    fn load_missing_and_malformed() {
        let dir = TempDir::new("missing");
        let missing = dir.path().join("none.json");
        assert_eq!(load_session(&missing).unwrap(), None);

        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{ not json").unwrap();
        assert!(load_session(&bad).is_err());
    }

    /// Пропуск несуществующих путей с пересчётом активной вкладки.
    #[test]
    fn plan_restore_skips_missing_files() {
        let dir = TempDir::new("plan");
        let a = dir.write("a.txt", "content\n");
        let b = dir.write("b.txt", "content\n");
        let gone1 = dir.path().join("gone1.txt");
        let gone2 = dir.path().join("gone2.txt");

        // [a, gone1, b, gone2], активная = b (индекс 2).
        let state = SessionState {
            tabs: vec![
                SessionTab { path: a.clone() },
                SessionTab { path: gone1.clone() },
                SessionTab { path: b.clone() },
                SessionTab { path: gone2.clone() },
            ],
            active_tab: Some(2),
            ..Default::default()
        };
        let plan = plan_restore(&state, |p| p.exists());
        assert_eq!(plan.tabs, vec![a.clone(), b.clone()]);
        assert_eq!(plan.skipped, vec![gone1, gone2]);
        assert_eq!(plan.active, Some(1)); // b стала индексом 1
    }

    /// Активная вкладка пропущена — активной становится ближайшая
    /// предыдущая выжившая.
    #[test]
    fn plan_restore_active_missing_falls_back_to_prev() {
        let dir = TempDir::new("plan-active-gone");
        let a = dir.write("a.txt", "content\n");
        let b = dir.write("b.txt", "content\n");
        let gone = dir.path().join("gone.txt");
        let state = SessionState {
            tabs: vec![
                SessionTab { path: a.clone() },
                SessionTab { path: b.clone() },
                SessionTab { path: gone.clone() },
            ],
            active_tab: Some(2),
            ..Default::default()
        };
        let plan = plan_restore(&state, |p| p.exists());
        assert_eq!(plan.tabs, vec![a, b]);
        assert_eq!(plan.skipped, vec![gone]);
        assert_eq!(plan.active, Some(1));
    }

    /// Все вкладки пропущены — пустой план, active = None.
    #[test]
    fn plan_restore_all_missing() {
        let gone = PathBuf::from("/definitely/not/here.txt");
        let state = SessionState {
            tabs: vec![SessionTab { path: gone.clone() }],
            active_tab: Some(0),
            ..Default::default()
        };
        let plan = plan_restore(&state, |_| false);
        assert!(plan.tabs.is_empty());
        assert_eq!(plan.skipped, vec![gone]);
        assert_eq!(plan.active, None);
    }

    /// Пустая сессия/только untitled — пустой план.
    #[test]
    fn plan_restore_empty_state() {
        let plan = plan_restore(&SessionState::default(), |_| true);
        assert_eq!(plan, RestorePlan::default());
    }

    /// Валидация геометрии.
    #[test]
    fn geometry_sanity() {
        assert!(WindowGeometry { x: 0.0, y: 0.0, width: 1000.0, height: 700.0 }.is_sane());
        assert!(!WindowGeometry { x: 0.0, y: 0.0, width: 100.0, height: 700.0 }.is_sane());
        assert!(!WindowGeometry { x: f32::NAN, y: 0.0, width: 1000.0, height: 700.0 }.is_sane());
    }

    /// app_data_dir — реальный путь в каталоге данных ОС (не хардкод
    /// home); на CI-машине каталог данных просто существует по контракту
    /// dirs::data_dir.
    #[test]
    fn app_data_dir_shape() {
        let p = app_data_dir().expect("dirs::data_dir available");
        assert!(p.ends_with(APP_DIR));
        assert!(session_path().unwrap().ends_with(SESSION_FILE));
    }
}
