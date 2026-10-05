//! Общая тест-инфраструктура editor-core (без внешних зависимостей):
//! временный каталог на процесс. До рефакторинга TempDir существовал
//! в двух местах (document.rs, encoding.rs) — теперь один.

use std::path::PathBuf;

pub(crate) struct TempDir(PathBuf);

impl TempDir {
    /// Уникальный временный каталог (tag + pid); существующий затирается.
    pub(crate) fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir()
            .join(format!("editor-core-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    /// Полный путь файла `name` внутри каталога.
    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// Записать файл, вернуть путь.
    pub(crate) fn write(&self, name: &str, content: &str) -> PathBuf {
        let path = self.path(name);
        std::fs::write(&path, content).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
