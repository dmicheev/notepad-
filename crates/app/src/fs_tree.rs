//! Модель дерева воркспейса (задача 7.1/7.2) — GPUI-free: корневая
//! папка + ленивое чтение детей (read_dir только раскрытых папок,
//! кэш на папку), сортировка «папки выше файлов» (внутри групп —
//! регистронезависимо по имени), операции создания/переименования/
//! удаления/перемещения и инвалидация по внешним FS-событиям (7.3,
//! `apply_external` — полный сброс кэша, D4-фолбэк).
//!
//! Скрытые записи (имя начинается с «.») не показываются — дефолт
//! MVP без настроек видимости. Симлинки не разворачиваются
//! (DirEntry::file_type не следует ссылке; симлинк на папку виден
//! как файл). Номеров строк нет — пути есть ключи всех состояний
//! (expanded/selected).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Запись каталога (ребро дерева).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

/// Плоская строка дерева для рендера (DFS по раскрытым папкам).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub expanded: bool,
    pub depth: usize,
}

/// Дерево воркспейса: корень + раскрытые папки + кэш детей.
#[derive(Debug)]
pub struct FsTree {
    root: PathBuf,
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    children_cache: HashMap<PathBuf, Rc<Vec<Entry>>>,
}

/// Проверка имени для create/rename: непусто, без разделителей,
/// не «.»/«..».
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

impl FsTree {
    /// Открыть воркспейс: папка обязана существовать (иначе io::Error).
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("workspace folder not found: {}", root.display()),
            ));
        }
        Ok(FsTree {
            root,
            expanded: HashSet::new(),
            selected: None,
            children_cache: HashMap::new(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Имя воркспейса для заголовка панели.
    pub fn root_name(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.display().to_string())
    }

    pub fn selected(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    /// Раскрытость папки (используется тестами — честное API модели,
    /// паттерн D10.4 «is_line_visible»).
    #[allow(dead_code)]
    pub fn is_expanded(&self, dir: &Path) -> bool {
        self.expanded.contains(dir)
    }

    /// Раскрыть/свернуть папку (кэш детей не сбрасывает — toggle
    /// только переключает флаг, чтение лениво по потребности).
    pub fn toggle(&mut self, dir: &Path) {
        if !self.expanded.remove(dir) {
            self.expanded.insert(dir.to_path_buf());
        }
    }

    /// Выделить узел (None — снять выделение).
    pub fn select(&mut self, path: Option<&Path>) {
        self.selected = path.map(|p| p.to_path_buf());
    }

    /// Каталоги, чьи дети уже в кэше (для тестов ленивости;
    /// паттерн D10.4).
    #[allow(dead_code)]
    pub fn cached_dirs(&self) -> Vec<&Path> {
        self.children_cache.keys().map(|p| p.as_path()).collect()
    }

    /// Дети каталога: из кэша или read_dir (сортировка — папки выше
    /// файлов, внутри групп регистронезависимо по имени; скрытые
    /// «.»-записи пропускаются). Ошибка чтения = пустой список
    /// (папка могла исчезнуть между событиями — рендер не должен
    /// падать).
    fn children(&mut self, dir: &Path) -> Rc<Vec<Entry>> {
        if let Some(cached) = self.children_cache.get(dir) {
            return cached.clone();
        }
        let mut entries: Vec<Entry> = match fs::read_dir(dir) {
            Ok(iter) => iter
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|n| !n.starts_with('.'))
                })
                .map(|e| {
                    let is_dir = e.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                    Entry {
                        name: e.file_name().to_string_lossy().into_owned(),
                        path: e.path(),
                        is_dir,
                    }
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let rc = Rc::new(entries);
        self.children_cache.insert(dir.to_path_buf(), rc.clone());
        rc
    }

    /// Плоский список видимых строк (DFS: дети раскрытой папки;
    /// корень всегда «раскрыт» — верхний уровень виден сразу).
    pub fn visible_rows(&mut self) -> Vec<Row> {
        let mut rows = Vec::new();
        let root = self.root.clone();
        self.walk(&root, 0, &mut rows);
        rows
    }

    fn walk(&mut self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        let children = self.children(dir);
        for entry in children.iter() {
            let expanded = entry.is_dir && self.expanded.contains(&entry.path);
            rows.push(Row {
                path: entry.path.clone(),
                name: entry.name.clone(),
                is_dir: entry.is_dir,
                expanded,
                depth,
            });
            if expanded {
                self.walk(&entry.path, depth + 1, rows);
            }
        }
    }

    // ---- Операции (7.2) ----

    /// Создать файл (пустой) в каталоге. Возвращает путь создания.
    pub fn create_file(&mut self, parent: &Path, name: &str) -> io::Result<PathBuf> {
        let path = Self::checked_child(parent, name)?;
        fs::write(&path, b"")?;
        self.refresh_dir(parent);
        Ok(path)
    }

    /// Создать подпапку. Возвращает путь создания.
    pub fn create_dir(&mut self, parent: &Path, name: &str) -> io::Result<PathBuf> {
        let path = Self::checked_child(parent, name)?;
        fs::create_dir(&path)?;
        self.refresh_dir(parent);
        Ok(path)
    }

    /// Переименовать узел (в том же каталоге). Возвращает новый путь;
    /// expanded/selected следуют за узлом.
    pub fn rename(&mut self, path: &Path, new_name: &str) -> io::Result<PathBuf> {
        let parent = path.parent().map(|p| p.to_path_buf()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "cannot rename root")
        })?;
        let new_path = Self::checked_child(&parent, new_name)?;
        if new_path.exists() {
            return Err(Self::exists_error(&new_path));
        }
        fs::rename(path, &new_path)?;
        self.follow_path(path, &new_path);
        self.refresh_dir(&parent);
        Ok(new_path)
    }

    /// Удалить узел: файл — remove_file, папка — remove_dir_all.
    pub fn delete(&mut self, path: &Path) -> io::Result<()> {
        if path == self.root {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cannot delete workspace root",
            ));
        }
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
        if let Some(parent) = path.parent() {
            self.refresh_dir(parent);
        }
        // Чистка состояний, указывающих внутрь удалённого поддерева.
        if self.selected.as_deref().is_some_and(|s| s == path || s.starts_with(path)) {
            self.selected = None;
        }
        self.expanded.retain(|e| e != path && !e.starts_with(path));
        self.children_cache.remove(path);
        Ok(())
    }

    /// Переместить узел в каталог (fs::rename). Гварды: не в себя, не
    /// в собственного потомка, не поверх существующего имени, только
    /// внутри воркспейса. Возвращает новый путь; expanded/selected
    /// следуют за узлом.
    pub fn move_into(&mut self, from: &Path, to_dir: &Path) -> io::Result<PathBuf> {
        if !from.starts_with(&self.root) || !to_dir.starts_with(&self.root) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "move is only supported inside the workspace",
            ));
        }
        if from == to_dir {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot move a folder into itself",
            ));
        }
        if to_dir.starts_with(from) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot move a folder into its own subtree",
            ));
        }
        let name = from.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "cannot move workspace root")
        })?;
        let new_path = to_dir.join(name);
        if new_path.exists() {
            return Err(Self::exists_error(&new_path));
        }
        fs::rename(from, &new_path)?;
        self.follow_path(from, &new_path);
        if let Some(old_parent) = from.parent() {
            self.refresh_dir(old_parent);
        }
        self.refresh_dir(to_dir);
        Ok(new_path)
    }

    /// Внешние изменения FS (7.3): полный сброс кэша (D4 fallback —
    /// correctness first; повторное чтение лениво по потребности),
    /// expanded/selected чистятся от исчезнувших путей.
    pub fn apply_external(&mut self) {
        self.children_cache.clear();
        self.expanded.retain(|p| p.is_dir());
        if self.selected.as_deref().is_some_and(|s| !s.exists()) {
            self.selected = None;
        }
    }

    // ---- Внутренние ----

    /// Проверить и собрать путь «родитель + имя» (валидность имени,
    /// отсутствие файла).
    fn checked_child(parent: &Path, name: &str) -> io::Result<PathBuf> {
        if !valid_name(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid name: {name:?}"),
            ));
        }
        let path = parent.join(name);
        if path.exists() {
            return Err(Self::exists_error(&path));
        }
        Ok(path)
    }

    fn exists_error(path: &Path) -> io::Error {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("\"{}\" already exists", path.display()),
        )
    }

    /// Сбросить кэш каталога (перечитается лениво при следующем
    /// visible_rows).
    fn refresh_dir(&mut self, dir: &Path) {
        self.children_cache.remove(dir);
    }

    /// Перевести expanded/selected со старого пути на новый
    /// (rename/move: узлы поддерева сохраняют состояние).
    fn follow_path(&mut self, old: &Path, new: &Path) {
        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
        for e in self.expanded.iter() {
            if e == old {
                moved.push((e.clone(), new.to_path_buf()));
            } else if let Ok(rest) = e.strip_prefix(old) {
                moved.push((e.clone(), new.join(rest)));
            }
        }
        for (from, to) in moved {
            self.expanded.remove(&from);
            self.expanded.insert(to);
        }
        if self.selected.as_deref() == Some(old) {
            self.selected = Some(new.to_path_buf());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn tree(tag: &str) -> (TempDir, FsTree) {
        let dir = TempDir::new(tag);
        dir.write("z-note.txt", "z\n");
        dir.write("a-note.md", "a\n");
        std::fs::create_dir_all(dir.path().join("Zeta")).unwrap();
        std::fs::create_dir_all(dir.path().join("alpha")).unwrap();
        dir.write("alpha/inner.txt", "in\n");
        dir.write(".hidden", "h\n");
        let t = FsTree::open(dir.path()).unwrap();
        (dir, t)
    }

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| r.name.clone()).collect()
    }

    /// Открытие несуществующей папки — ошибка.
    #[test]
    fn open_requires_dir() {
        let dir = TempDir::new("open-miss");
        assert!(FsTree::open(dir.path().join("nope")).is_err());
    }

    /// Сортировка: папки выше файлов, регистронезависимо; скрытые
    /// записи не показываются; верхний уровень виден без кликов.
    #[test]
    fn rows_sorted_folders_first() {
        let (_dir, mut t) = tree("sort");
        let rows = t.visible_rows();
        assert_eq!(
            names(&rows),
            vec!["alpha".to_string(), "Zeta".to_string(), "a-note.md".to_string(), "z-note.txt".to_string()]
        );
        assert!(rows.iter().all(|r| r.depth == 0));
    }

    /// Ленивое чтение: дети вложенной папки читаются только после
    /// раскрытия (кэш содержит ключи только прочитанных каталогов).
    #[test]
    fn lazy_children_read_on_expand() {
        let (_dir, mut t) = tree("lazy");
        t.visible_rows();
        // Прочитан только корень.
        assert_eq!(t.cached_dirs().len(), 1);
        // Раскрытие alpha: lazy — сам read_dir откладывается до
        // следующего visible_rows.
        let alpha = t.root().join("alpha");
        t.toggle(&alpha);
        assert_eq!(t.cached_dirs().len(), 1);
        let rows = t.visible_rows();
        assert_eq!(t.cached_dirs().len(), 2);
        let inner = rows.iter().find(|r| r.name == "inner.txt").unwrap();
        assert_eq!(inner.depth, 1);
        // Сворачивание прячет детей, кэш остаётся (повторное раскрытие
        // бесплатное).
        t.toggle(&alpha);
        assert!(!t.visible_rows().iter().any(|r| r.name == "inner.txt"));
        t.toggle(&alpha);
        assert!(t.visible_rows().iter().any(|r| r.name == "inner.txt"));
    }

    /// Создание файла/папки появляется в дереве и на диске.
    #[test]
    fn create_file_and_dir() {
        let (_dir, mut t) = tree("create");
        let sub = t.root().join("alpha");
        t.create_dir(&sub, "nested").unwrap();
        let f = t.create_file(&sub, "page.txt").unwrap();
        assert!(f.is_file());
        t.toggle(&sub);
        let rows = t.visible_rows();
        let nested = rows.iter().find(|r| r.name == "nested").unwrap();
        assert!(nested.is_dir);
        assert!(rows.iter().any(|r| r.name == "page.txt"));
        // Дубликат имени и пустое имя — ошибки.
        assert!(t.create_file(&sub, "page.txt").is_err());
        assert!(t.create_file(&sub, "").is_err());
        assert!(t.create_file(&sub, "a/b").is_err());
    }

    /// Переименование: узел в строках меняется, выделение следует.
    #[test]
    fn rename_updates_rows_and_selection() {
        let (_dir, mut t) = tree("rename");
        let old = t.root().join("z-note.txt");
        t.select(Some(&old));
        let new = t.rename(&old, "renamed.txt").unwrap();
        assert_eq!(new, t.root().join("renamed.txt"));
        assert!(!old.exists());
        assert!(new.is_file());
        assert_eq!(t.selected(), Some(new.as_path()));
        let rows = t.visible_rows();
        assert!(rows.iter().any(|r| r.path == new));
        assert!(!rows.iter().any(|r| r.path == old));
        // Поверх существующего имени — ошибка.
        assert!(t.rename(&new, "a-note.md").is_err());
    }

    /// Переименованная раскрытая папка сохраняет раскрытость.
    #[test]
    fn rename_keeps_expanded_subtree() {
        let (_dir, mut t) = tree("rename-dir");
        let alpha = t.root().join("alpha");
        t.toggle(&alpha);
        t.visible_rows();
        let new = t.rename(&alpha, "beta").unwrap();
        assert!(t.is_expanded(&new));
        assert!(t.visible_rows().iter().any(|r| r.name == "inner.txt"));
    }

    /// Удаление: файла, папки с содержимым; выделение/раскрытость
    /// внутри удалённого поддерева чистятся.
    #[test]
    fn delete_file_and_dir() {
        let (_dir, mut t) = tree("delete");
        let note = t.root().join("z-note.txt");
        t.select(Some(&note));
        t.delete(&note).unwrap();
        assert!(!note.exists());
        assert_eq!(t.selected(), None);

        let alpha = t.root().join("alpha");
        t.toggle(&alpha);
        t.select(Some(&alpha.join("inner.txt")));
        t.delete(&alpha).unwrap();
        assert!(!alpha.exists());
        assert!(!t.is_expanded(&alpha));
        assert_eq!(t.selected(), None);
        assert!(!t.visible_rows().iter().any(|r| r.name == "alpha"));
    }

    /// Перемещение файла в папку: на диске, в строках; гварды —
    /// в себя / в потомка / поверх существующего имени.
    #[test]
    fn move_into_folder_with_guards() {
        let (_dir, mut t) = tree("move");
        let note = t.root().join("z-note.txt");
        let alpha = t.root().join("alpha");
        let moved = t.move_into(&note, &alpha).unwrap();
        assert_eq!(moved, alpha.join("z-note.txt"));
        assert!(moved.is_file());
        assert!(!note.exists());
        t.toggle(&alpha);
        assert!(t.visible_rows().iter().any(|r| r.path == moved));

        // Поверх существующего имени: новый z-note.txt в корне vs
        // уже лежащий в alpha z-note.txt.
        let root = t.root().to_path_buf();
        let again = t.create_file(&root, "z-note.txt").unwrap();
        assert!(t.move_into(&again, &alpha).is_err());

        // Папку в собственного потомка / в себя — ошибка.
        let zeta = t.root().join("Zeta");
        let nested = t.create_dir(&zeta, "sub").unwrap();
        assert!(t.move_into(&zeta, &nested).is_err());
        assert!(t.move_into(&zeta, &zeta).is_err());
    }

    /// Перемещение раскрытой папки сохраняет раскрытость и детей
    /// (видимость — при раскрытой папке-приёмнике).
    #[test]
    fn move_dir_keeps_expanded() {
        let (_dir, mut t) = tree("move-dir");
        let alpha = t.root().join("alpha");
        let zeta = t.root().join("Zeta");
        t.toggle(&alpha);
        t.toggle(&zeta);
        let new = t.move_into(&alpha, &zeta).unwrap();
        assert_eq!(new, zeta.join("alpha"));
        assert!(t.is_expanded(&new));
        assert!(t.visible_rows().iter().any(|r| r.name == "inner.txt"));
    }

    /// Внешние изменения (7.3): после apply_external новые файлы
    /// видны, исчезнувшие папки выпадают из expanded/selected.
    #[test]
    fn apply_external_refreshes() {
        let (dir, mut t) = tree("external");
        let alpha = dir.path().join("alpha");
        t.toggle(&alpha);
        let inner = alpha.join("inner.txt");
        t.select(Some(&inner));
        t.visible_rows();

        // Извне: создан файл, папка переименована (старая исчезла).
        dir.write("outside.txt", "o\n");
        std::fs::rename(&alpha, dir.path().join("beta")).unwrap();

        t.apply_external();
        let rows = t.visible_rows();
        assert!(rows.iter().any(|r| r.name == "outside.txt"));
        assert!(rows.iter().any(|r| r.name == "beta"));
        assert!(!rows.iter().any(|r| r.name == "alpha"));
        assert!(!t.is_expanded(&alpha)); // старого пути больше нет
        assert_eq!(t.selected(), None); // inner.txt исчез
    }

    /// Симлинк не разворачивается как папка (file_type без follow).
    #[test]
    fn symlink_is_file_row() {
        let (dir, mut t) = tree("symlink");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("alpha"), dir.path().join("link")).unwrap();
        #[cfg(not(unix))]
        let _ = dir; // платформы без unix-симлинков: тест вырожден
        let rows = t.visible_rows();
        let link = rows.iter().find(|r| r.name == "link");
        #[cfg(unix)]
        assert!(link.is_some_and(|r| !r.is_dir));
        #[cfg(not(unix))]
        let _ = link;
    }
}
