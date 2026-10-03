//! Общая тест-инфраструктура app (без внешних зависимостей): временный
//! каталог на процесс. До рефакторинга TempDir существовал в трёх
//! местах (tab_model, session, gui_tests) — теперь один.

use std::path::{Path, PathBuf};

pub(crate) struct TempDir(PathBuf);

impl TempDir {
    /// Уникальный временный каталог (tag + pid); существующий затирается.
    pub(crate) fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir()
            .join(format!("app-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    /// Записать файл, вернуть полный путь.
    pub(crate) fn write(&self, name: &str, content: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    /// Корень каталога.
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
