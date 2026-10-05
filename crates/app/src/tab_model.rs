//! Модель вкладок (задача 3.2) — GPUI-free: открытие/закрытие/
//! переключение/перестановка, активный индекс, dirty-флаги,
//! untitled-нумерация. Содержимое одной вкладки (документ/курсоры/
//! язык/подсветка) — buffer_tab.rs.

use std::io;
use std::path::Path;

use crate::buffer_tab::BufferTab;


/// Модель вкладок: открытие/закрытие/переключение/перестановка,
/// активный индекс, dirty-флаги, untitled-нумерация.
#[derive(Debug, Default)]
pub struct TabModel {
    tabs: Vec<BufferTab>,
    active: Option<usize>,
    next_untitled: u32,
}

impl TabModel {
    pub fn new() -> Self {
        TabModel::default()
    }

    pub fn tabs(&self) -> &[BufferTab] {
        &self.tabs
    }

    /// Мутабельный обход вкладок (прогрев языков всех панелей, 8.1).
    pub fn tabs_mut(&mut self) -> impl Iterator<Item = &mut BufferTab> {
        self.tabs.iter_mut()
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn active_index(&self) -> Option<usize> {
        self.active
    }

    pub fn active(&self) -> Option<&BufferTab> {
        self.active.and_then(|i| self.tabs.get(i))
    }

    pub fn active_mut(&mut self) -> Option<&mut BufferTab> {
        let i = self.active?;
        self.tabs.get_mut(i)
    }

    /// Мутабельный доступ к вкладке по индексу (для save-цепочек).
    pub fn tab_mut(&mut self, index: usize) -> Option<&mut BufferTab> {
        self.tabs.get_mut(index)
    }

    /// Заголовок вкладки по индексу (untitled-номер вычисляется по
    /// порядку среди untitled-вкладок).
    pub fn title(&self, index: usize) -> Option<String> {
        let tab = self.tabs.get(index)?;
        let no = self.untitled_no(index);
        Some(tab.title(no))
    }

    fn untitled_no(&self, index: usize) -> u32 {
        let mut no = 0;
        for (i, t) in self.tabs.iter().enumerate() {
            if t.path.is_none() {
                no += 1;
                if i >= index {
                    break;
                }
            }
        }
        no.max(1)
    }

    /// Открыть файл: если путь уже открыт — активировать существующую
    /// вкладку (дедупликация без каноникализации — диалоги ОС дают
    /// абсолютные нормализованные пути; D8.2), иначе новую вкладку
    /// и активировать её. Ошибка загрузки — наружу (GUI покажет).
    /// Длительность успешного открытия идёт в панель PERFORMANCE (D11.8).
    pub fn open_path(&mut self, path: impl AsRef<Path>) -> io::Result<usize> {
        let path = path.as_ref();
        if let Some(i) = self.tabs.iter().position(|t| t.path.as_deref() == Some(path)) {
            self.active = Some(i);
            return Ok(i);
        }
        let started = std::time::Instant::now();
        let tab = BufferTab::open(path)?;
        if let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) {
            crate::perf::note_open(name, started.elapsed().as_secs_f64() * 1000.0);
        }
        self.tabs.push(tab);
        let i = self.tabs.len() - 1;
        self.active = Some(i);
        Ok(i)
    }

    /// Перепривязать вкладку файла к новому пути (7.2: файл
    /// переименован/перемещен в дереве — открытая вкладка остаётся
    /// привязанной к новому пути, включая последующие сохранения).
    /// Возвращает, была ли переброшена хоть одна вкладка.
    pub fn retag_path(&mut self, old: &Path, new: &Path) -> bool {
        let mut hit = false;
        for tab in &mut self.tabs {
            if tab.path.as_deref() == Some(old) {
                tab.path = Some(new.to_path_buf());
                hit = true;
            }
        }
        hit
    }

    /// Новая untitled-вкладка (активируется).
    pub fn new_untitled(&mut self) -> usize {
        self.next_untitled += 1;
        self.tabs.push(BufferTab::untitled());
        let i = self.tabs.len() - 1;
        self.active = Some(i);
        i
    }

    /// Активировать вкладку (границы — no-op).
    pub fn activate(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = Some(index);
        }
    }

    /// Закрыть вкладку по индексу. Активной становится правая соседка,
    /// если закрыли последнюю — левая; закрытие активной последней
    /// вкладки оставляет active = None.
    pub fn close(&mut self, index: usize) -> Option<BufferTab> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.active = None;
        } else if let Some(a) = self.active {
            self.active = Some(if index < a {
                a - 1
            } else if index == a {
                a.min(self.tabs.len() - 1)
            } else {
                a
            });
        }
        Some(tab)
    }

    /// Перестановка: сдвинуть активную вкладку на `offset` позиций
    /// (кнопки ‹ › в таб-баре, D8.2; drag-and-drop — позже). Возвращает
    /// новый индекс активной вкладки.
    pub fn move_active(&mut self, offset: isize) -> Option<usize> {
        let from = self.active?;
        let to = if offset < 0 {
            from.saturating_sub(offset.unsigned_abs())
        } else {
            (from + offset.unsigned_abs()).min(self.tabs.len() - 1)
        };
        if from == to {
            return Some(from);
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = Some(to);
        Some(to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use editor_core::Selection;

    #[test]
    fn open_switch_close() {
        let dir = TempDir::new("open-switch-close");
        let a = dir.write("a.txt", "alpha\n");
        let b = dir.write("b.txt", "beta\n");
        let c = dir.write("c.txt", "gamma\n");

        let mut m = TabModel::new();
        assert!(m.is_empty());
        assert_eq!(m.active_index(), None);

        m.open_path(&a).unwrap();
        m.open_path(&b).unwrap();
        m.open_path(&c).unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m.active_index(), Some(2));
        assert_eq!(m.title(2).unwrap(), "c.txt");

        m.activate(0);
        assert_eq!(m.active_index(), Some(0));
        m.activate(99); // вне диапазона — no-op
        assert_eq!(m.active_index(), Some(0));

        // Закрытие средней (не активной) — активная смещается влево.
        m.close(1);
        assert_eq!(m.len(), 2);
        assert_eq!(m.active_index(), Some(0));
        assert_eq!(m.title(1).unwrap(), "c.txt");

        // Закрытие активной (0) — активной становится правая соседка.
        m.close(0);
        assert_eq!(m.len(), 1);
        assert_eq!(m.active_index(), Some(0));
        assert_eq!(m.title(0).unwrap(), "c.txt");

        // Последняя вкладка: active -> None.
        m.close(0);
        assert!(m.is_empty());
        assert_eq!(m.active_index(), None);
        assert!(m.active().is_none());
    }

    /// Повторное открытие того же пути активирует существующую вкладку.
    #[test]
    fn reopen_same_path_activates_existing_tab() {
        let dir = TempDir::new("dedup");
        let a = dir.write("a.txt", "content\n");
        let mut m = TabModel::new();
        m.open_path(&a).unwrap();
        m.new_untitled();
        let i = m.open_path(&a).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m.active_index(), Some(0));
        assert_eq!(i, 0);
    }

    /// 7.2: переименованный/перемещённый файл — вкладка следует за
    /// новым путём (сохранение пишет уже в новое место).
    #[test]
    fn retag_path_follows_moved_file() {
        let dir = TempDir::new("retag");
        let a = dir.write("a.txt", "content\n");
        let sub = dir.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let mut m = TabModel::new();
        m.open_path(&a).unwrap();
        let moved = sub.join("a.txt");

        assert!(m.retag_path(&a, &moved));
        assert!(!m.retag_path(&a, &moved)); // старый путь больше не открыт
        assert_eq!(m.active().unwrap().path.as_deref(), Some(moved.as_path()));
        // Повторное «открытие» старого пути создаёт НОВУЮ вкладку —
        // дедупликация смотрит на актуальные пути.
        assert_eq!(m.open_path(&a).unwrap(), 1);

        // Сохранение уходит в новый путь.
        m.activate(0);
        m.active_mut().unwrap().insert_text("more");
        m.active_mut().unwrap().save().unwrap();
        assert!(moved.is_file());
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "morecontent\n");
    }

    /// Untitled-вкладки: нумерация заголовков по порядку.
    #[test]
    fn untitled_tabs_numbering() {
        let dir = TempDir::new("untitled");
        let a = dir.write("a.txt", "x");
        let mut m = TabModel::new();
        m.new_untitled();
        m.new_untitled();
        m.open_path(&a).unwrap();
        m.new_untitled();
        assert_eq!(m.title(0).unwrap(), "Untitled 1");
        assert_eq!(m.title(1).unwrap(), "Untitled 2");
        assert_eq!(m.title(2).unwrap(), "a.txt");
        assert_eq!(m.title(3).unwrap(), "Untitled 3");

        // Закрытие untitled 1 перенумерует оставшиеся.
        m.close(0);
        assert_eq!(m.title(0).unwrap(), "Untitled 1"); // бывшая Untitled 2
        assert_eq!(m.title(2).unwrap(), "Untitled 2"); // бывшая 3
    }

    /// Untitled-вкладка: пустой документ, UTF-8/LF (дефолт FileFormat).
    #[test]
    fn untitled_tab_is_utf8_lf() {
        let t = BufferTab::untitled();
        assert_eq!(t.doc.text(), "");
        assert_eq!(t.doc.encoding().label(), "UTF-8");
        assert_eq!(t.doc.line_endings().dominant().label(), "LF");
        assert!(!t.dirty);
        assert_eq!(*t.primary(), Selection::caret(0));
    }

    /// Перестановка вкладок кнопками (move_active), границы — no-op.
    #[test]
    fn reorder_tabs() {
        let dir = TempDir::new("reorder");
        let a = dir.write("a.txt", "a");
        let b = dir.write("b.txt", "b");
        let c = dir.write("c.txt", "c");
        let mut m = TabModel::new();
        for p in [&a, &b, &c] {
            m.open_path(p).unwrap();
        }
        assert_eq!(m.active_index(), Some(2));

        assert_eq!(m.move_active(-1), Some(1)); // c <-> b
        assert_eq!(
            m.tabs().iter().map(m_title).collect::<Vec<_>>(),
            vec!["a.txt", "c.txt", "b.txt"]
        );
        assert_eq!(m.active_index(), Some(1));
        assert_eq!(m.active().unwrap().path.as_deref(), Some(c.as_path()));

        m.move_active(5); // clamp в конец
        assert_eq!(m.active_index(), Some(2));
        m.move_active(-5); // clamp в начало — активная (c) уезжает в начало
        assert_eq!(m.active_index(), Some(0));
        assert_eq!(m_title(&m.tabs()[0]), "c.txt");
        assert_eq!(
            m.tabs().iter().map(m_title).collect::<Vec<_>>(),
            vec!["c.txt", "a.txt", "b.txt"]
        );

        // Перестановка неактивной вкладки не предусмотрена (кнопки
        // двигают активную) — активная следует за вкладкой.
    }

    fn m_title(t: &BufferTab) -> String {
        t.title(0)
    }

    /// Dirty: правка ставит флаг, успешное сохранение сбрасывает;
    /// save_as запоминает новый путь.
    #[test]
    fn dirty_flag_lifecycle() {
        let dir = TempDir::new("dirty");
        let a = dir.write("a.txt", "hello");
        let mut m = TabModel::new();
        m.open_path(&a).unwrap();
        assert!(!m.active().unwrap().dirty);

        // Ввод в конец файла (каретка после открытия в начале).
        let tab = m.active_mut().unwrap();
        tab.set_caret_at(0, tab.doc.line_content(0).chars().count());
        tab.insert_text("!");
        assert!(m.active().unwrap().dirty);
        assert_eq!(m.active().unwrap().doc.text(), "hello!");

        // Сохранение сбрасывает.
        m.active_mut().unwrap().save().unwrap();
        assert!(!m.active().unwrap().dirty);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "hello!");

        // Save-as на untitled: путь запоминается, dirty сбрасывается.
        m.new_untitled();
        m.active_mut().unwrap().insert_text("new file");
        let target = dir.path().join("saved.txt");
        m.active_mut().unwrap().save_to(&target).unwrap();
        assert!(!m.active().unwrap().dirty);
        assert_eq!(m.active().unwrap().path.as_deref(), Some(target.as_path()));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new file");
    }

    /// Загрузка файла: dirty=false, история пуста, каретка в начале.
    #[test]
    fn opened_file_starts_clean() {
        let dir = TempDir::new("clean-open");
        let a = dir.write("a.txt", "line\n");
        let mut m = TabModel::new();
        m.open_path(&a).unwrap();
        let t = m.active().unwrap();
        assert!(!t.dirty);
        assert!(!t.history.can_undo());
        assert_eq!(*t.primary(), Selection::caret(0));
        assert_eq!(t.doc.text(), "line\n");
    }

    #[test]
    fn title_is_file_name() {
        let dir = TempDir::new("title");
        let nested = dir.path().join("sub");
        std::fs::create_dir_all(&nested).unwrap();
        let p = nested.join("deep.rs");
        std::fs::write(&p, "fn main() {}").unwrap();
        let mut m = TabModel::new();
        m.open_path(&p).unwrap();
        assert_eq!(m.title(0).unwrap(), "deep.rs");
    }

    // ---- Подсветка синтаксиса (группа 5): сценарии спеки ----

    use editor_core::highlight::CLASS_COMMENT;

    /// Сценарий спеки «Открытие Python-файла»: .py получает подсветку
    /// Python без действий пользователя (прогрев уже отработал —
    /// refresh_language детектит по расширению).
    #[test]
    fn python_file_auto_highlighted() {
        let dir = TempDir::new("py-auto");
        let path = dir.write("script.py", "def main():\n    # comment\n    pass\n");
        let mut m = TabModel::new();
        m.open_path(&path).unwrap();
        let tab = m.active_mut().unwrap();
        tab.refresh_language(); // путь прогрева
        assert_eq!(tab.language_label(), "Python");
        assert_eq!(tab.highlight.syntax_name(), Some("Python"));
        // Движок красит комментарий уже после первого окна.
        tab.highlight.set_budget(std::time::Duration::from_secs(1));
        tab.highlight.ensure_window(&tab.doc, 0, 3);
        let spans = tab.highlight.spans(1).expect("спаны строки 1");
        assert_eq!(
            spans.iter().map(|s| (s.start, s.end, s.class)).collect::<Vec<_>>(),
            vec![(4, 13, CLASS_COMMENT)]
        );
    }

    /// Сценарий спеки «Ручной выбор языка»: файл БЕЗ расширения,
    /// выбор YAML — подсветка применяется немедленно.
    #[test]
    fn manual_yaml_on_extensionless_file() {
        let dir = TempDir::new("yaml-manual");
        let path = dir.write("data", "# comment\nkey: value\n");
        let mut m = TabModel::new();
        m.open_path(&path).unwrap();
        let tab = m.active_mut().unwrap();
        tab.refresh_language();
        assert_eq!(tab.language_label(), "Plain Text", "без расширения — авто нет");
        assert_eq!(tab.highlight.syntax_name(), None);
        // Ручной выбор YAML (оверлей 5.1 разрешает имя в грамматику).
        let yaml = crate::language::syntax_for_choice("YAML").unwrap();
        tab.set_language(Some(yaml));
        assert_eq!(tab.language_label(), "YAML");
        assert_eq!(tab.highlight.syntax_name(), Some("YAML"));
        // Подсветка применяется НЕМЕДЛЕННО (кэш сброшен, окно красится).
        tab.highlight.set_budget(std::time::Duration::from_secs(1));
        tab.highlight.ensure_window(&tab.doc, 0, 2);
        let spans = tab.highlight.spans(0).expect("спаны строки 0");
        assert_eq!(
            spans.iter().map(|s| (s.start, s.end, s.class)).collect::<Vec<_>>(),
            vec![(0, 9, CLASS_COMMENT)]
        );
    }

    /// Сценарий спеки «Файл сверх порога»: 20 МБ+1 байт открывается
    /// редактируемым БЕЗ подсветки; язык «Plain Text»; undo работает.
    #[test]
    fn over_threshold_file_editable_without_highlight() {
        let dir = TempDir::new("over-threshold");
        // 20 МиБ + 1 байт текста (короткие строки — быстрее собирается).
        let line = "abcdefghij\n";
        let mut text = String::with_capacity(20 * 1024 * 1024 + line.len());
        while text.len() <= 20 * 1024 * 1024 {
            text.push_str(line);
        }
        assert!(text.len() > crate::language::HIGHLIGHT_MAX_BYTES as usize);
        let path = dir.write("big.rs", &text); // .rs! язык всё равно отключён
        let mut m = TabModel::new();
        m.open_path(&path).unwrap();
        let tab = m.active_mut().unwrap();
        assert!(tab.language.disabled());
        assert_eq!(tab.language_label(), "Plain Text");
        assert_eq!(tab.highlight.syntax_name(), None);
        tab.refresh_language(); // порог необратим в рамках вкладки
        assert_eq!(tab.highlight.syntax_name(), None);
        // Редактирование полноценно: ввод/undo.
        tab.insert_text("X");
        assert_eq!(tab.doc.slice_str(0, 1), "X");
        assert!(tab.undo());
        assert_eq!(tab.doc.slice_str(0, 1), "a");
        // Ровно 20 МиБ — подсветка ЕЩЁ включена (порог «свыше»).
        let exact: String = "x".repeat(20 * 1024 * 1024);
        let p2 = dir.write("exact.rs", &exact);
        m.open_path(&p2).unwrap();
        let t2 = m.active_mut().unwrap();
        t2.refresh_language();
        assert!(!t2.language.disabled(), "ровно порог — не отключать");
        assert_eq!(t2.language_label(), "Rust");
    }

    /// Инвалидация правки через вкладку: спаны до строки правки живут,
    /// после — пересчитываются; undo сбрасывает кэш (ленивый переразбор).
    #[test]
    fn highlight_cache_follows_edits() {
        let dir = TempDir::new("hl-edits");
        let path = dir.write("a.py", "# one\n# two\n# three\n");
        let mut m = TabModel::new();
        m.open_path(&path).unwrap();
        let tab = m.active_mut().unwrap();
        tab.refresh_language();
        tab.highlight.set_budget(std::time::Duration::from_secs(1));
        tab.highlight.ensure_window(&tab.doc, 0, 3);
        assert!(tab.highlight.spans(2).is_some());
        // Правка строки 1: кэш строки 0 жив, 1+ пересчитается.
        tab.set_caret_at(1, 0);
        tab.insert_text("x = 1\n");
        assert!(tab.highlight.spans(0).is_some(), "строка выше правки жива");
        tab.highlight.ensure_window(&tab.doc, 0, 4);
        // «x = 1»: '=' keyword.operator, '1' — число; комментария нет
        // (до правки строка была комментарием — кэш пересчитан).
        let spans1 = tab.highlight.spans(1).expect("пересчитана");
        assert!(
            spans1.iter().all(|s| s.class != CLASS_COMMENT),
            "{spans1:?}"
        );
        assert!(spans1.iter().any(|s| s.class == editor_core::highlight::CLASS_NUMBER));
        // Undo: полный сброс — окно пересчитывается заново корректно.
        assert!(tab.undo());
        tab.highlight.ensure_window(&tab.doc, 0, 3);
        assert_eq!(
            tab.highlight
                .spans(1)
                .unwrap()
                .iter()
                .map(|s| (s.start, s.end, s.class))
                .collect::<Vec<_>>(),
            vec![(0, 5, CLASS_COMMENT)]
        );
    }

    /// Каретки/выделения кэш подсветки не трогают (D12.1).
    #[test]
    fn caret_moves_do_not_invalidate_highlight() {
        let dir = TempDir::new("hl-caret");
        let path = dir.write("a.py", "# c\n");
        let mut m = TabModel::new();
        m.open_path(&path).unwrap();
        let tab = m.active_mut().unwrap();
        tab.refresh_language();
        tab.highlight.set_budget(std::time::Duration::from_secs(1));
        tab.highlight.ensure_window(&tab.doc, 0, 1);
        let before = tab.highlight.spans(0).unwrap().clone();
        tab.set_caret_at(0, 1);
        tab.select_to(0, 1);
        assert!(!tab.undo()); // движений без правок — нет undo
        assert_eq!(tab.highlight.spans(0), Some(&before));
    }

    /// Ручной выбор → сброс в Auto возвращает детект; Plain Text гасит.
    #[test]
    fn language_override_cycle() {
        let dir = TempDir::new("hl-cycle");
        let path = dir.write("m.md", "# title\n");
        let mut m = TabModel::new();
        m.open_path(&path).unwrap();
        let tab = m.active_mut().unwrap();
        tab.refresh_language();
        assert_eq!(tab.language_label(), "Markdown");
        let yaml = crate::language::syntax_for_choice("YAML").unwrap();
        tab.set_language(Some(yaml));
        assert_eq!(tab.language_label(), "YAML");
        tab.set_language_auto();
        assert_eq!(tab.language_label(), "Markdown");
        tab.set_language(None); // Plain Text
        assert_eq!(tab.language_label(), "Plain Text");
        assert_eq!(tab.highlight.syntax_name(), None);
    }

    /// Save-as меняет расширение → авто-детект по новому пути (5.1).
    #[test]
    fn save_as_redetects_language() {
        let dir = TempDir::new("hl-saveas");
        let path = dir.write("untitled-ish", "# comment\n");
        let mut m = TabModel::new();
        m.open_path(&path).unwrap();
        let tab = m.active_mut().unwrap();
        tab.refresh_language();
        assert_eq!(tab.language_label(), "Plain Text");
        let target = dir.path().join("saved.py");
        tab.save_to(&target).unwrap();
        assert_eq!(tab.language_label(), "Python");
    }

    /// D12.5: is_csv — грамматика «CSV» (детект/ручной выбор) ИЛИ
    /// расширение пути .csv/.tsv (регистронезависимо; покрывает
    /// порог-отключение подсветки — шапка липнет без спанов).
    /// Детект холодного/тёплого набора не меняет результат: холодный
    /// набор даёт None-грамматику → срабатывает расширение пути.
    #[test]
    fn is_csv_for_csv_tsv_and_non_csv() {
        let dir = TempDir::new("is-csv");
        let csv = dir.write("a.csv", "x,y\n");
        let tsv = dir.write("b.tsv", "x\ty\n");
        let upper = dir.write("C.CSV", "x,y\n");
        let txt = dir.write("c.txt", "x,y\n");
        assert!(BufferTab::open(&csv).unwrap().is_csv());
        assert!(BufferTab::open(&tsv).unwrap().is_csv());
        assert!(BufferTab::open(&upper).unwrap().is_csv(), "регистронезависимо");
        assert!(!BufferTab::open(&txt).unwrap().is_csv());
        // Untitled (пути нет, грамматики нет) — не CSV.
        assert!(!BufferTab::untitled().is_csv());
        // Ручной выбор грамматики CSV для произвольного файла.
        let mut tab = BufferTab::open(&txt).unwrap();
        tab.set_language(crate::language::syntax_for_choice("CSV"));
        assert!(tab.is_csv());
    }}
