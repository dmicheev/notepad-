//! BufferTab — одна вкладка (задача 3.2) — GPUI-free: документ,
//! история, курсоры (мульти-курсор, главный — последний) и dirty-флаг;
//! редактирование идёт ТОЛЬКО через editor-core (Document + History +
//! транзакции selection.rs), каретка смещается правками через
//! `Transaction::map_selection`. Список вкладок — tab_model.rs.
//!
//! Подсветка (группа 5): вкладка владеет языком (авто-детект + ручной
//! override + порог 20 МБ) и движком `HighlightEngine` (кэш спанов на
//! строку, чекпоинты стеков); инвалидация — в `apply_tx`/undo/redo.
//! Горизонтальный скролл: `max_seen_cols` — ракетка ширины рендера.

use std::io;
use std::path::PathBuf;

use editor_core::{Document, History, HighlightEngine, Selection, SyntaxReference};
use editor_core::{backspace, forward_delete, insert_text};

/// Ручной выбор языка (5.1): авто / Plain Text / конкретная грамматика.
#[derive(Debug, Default)]
pub enum LanguageOverride {
    #[default]
    Auto,
    PlainText,
    Syntax(&'static SyntaxReference),
}

/// Язык вкладки: авто-детект (грамматика + метка статус-бара), ручной
/// override, порог 20 МБ. Метка ≠ имя грамматики для TypeScript
/// (псевдоним JS — D12.3). Порог (правка спеки, окт 2026/D12.4):
/// файл больше 20 МБ открывается БЕЗ подсветки, приложение спрашивает
/// пользователя; согласие (`threshold_allowed`) включает подсветку
/// для любого размера.
#[derive(Debug, Default)]
pub struct TabLanguage {
    auto: Option<crate::language::LabeledSyntax>,
    pub manual: LanguageOverride,
    /// Файл больше `HIGHLIGHT_MAX_BYTES` (байтов) — подсветка
    /// выключена, пока пользователь не разрешит (диалог).
    pub over_threshold: bool,
    /// Пользователь разрешил подсветку сверх порога.
    pub threshold_allowed: bool,
}

impl TabLanguage {
    /// Подсветка выключена для этой вкладки.
    pub fn disabled(&self) -> bool {
        self.over_threshold && !self.threshold_allowed
    }

    /// Метка для статус-бара.
    pub fn label(&self) -> &'static str {
        if self.disabled() {
            return "Plain Text";
        }
        match &self.manual {
            LanguageOverride::Syntax(s) => {
                editor_core::syntax::friendly_name(&s.name)
            }
            LanguageOverride::PlainText => "Plain Text",
            LanguageOverride::Auto => {
                self.auto.as_ref().map(|(_, l)| *l).unwrap_or("Plain Text")
            }
        }
    }

    /// Грамматика подсветки; `None` — без подсветки.
    pub fn effective(&self) -> Option<&'static SyntaxReference> {
        if self.disabled() {
            return None;
        }
        match &self.manual {
            LanguageOverride::Syntax(s) => Some(s),
            LanguageOverride::PlainText => None,
            LanguageOverride::Auto => self.auto.as_ref().map(|(s, _)| *s),
        }
    }
}

/// Одна вкладка: документ + история + курсоры + dirty-флаг
/// (dirty ведётся здесь, на стороне tab-модели; editor-core не
/// расширяется — design.md D8.2).
#[derive(Debug)]
pub struct BufferTab {
    /// `None` — untitled-вкладка (первый Cmd+S → диалог сохранения).
    pub path: Option<PathBuf>,
    pub doc: Document,
    pub history: History,
    /// Курсоры (4.3): каретки и выделения. ГЛАВНЫЙ — ПОСЛЕДНИЙ элемент
    /// (добавление Cmd+клик/Cmd+D аппендит; D9.3).
    pub cursors: Vec<Selection>,
    /// Цель по столбцу для вертикальной навигации (4.2, D9.2):
    /// запоминается при первом движении вверх/вниз, сбрасывается любым
    /// горизонтальным движением/кликом/вводом. Только для одного курсора.
    pub goal_col: Option<usize>,
    pub dirty: bool,
    /// Scroll-смещение редактора (y, px) — для переключений вкладок
    /// и восстановления сессии (по желанию, D8.4).
    pub scroll_y: f32,
    /// Самая широкая строка (char-колонок), ВИДЕННАЯ в этой вкладке
    /// (горизонтальный скролл редактора): точный максимум на open,
    /// дальше — ракетка рендера (ширина контента только растёт;
    /// укорочение строк не сжимает полосу доreopen — сознательный
    /// MVP-компромисс). Табы в кодировках не моно — см. рендер.
    max_seen_cols: usize,
    /// Язык + подсветка (группа 5).
    pub language: TabLanguage,
    /// Движок подсветки: кэши спанов/стеков видимых строк (D12.1).
    pub highlight: HighlightEngine,
    /// Версия вида: растёт при любом изменении, влияющем на рендер
    /// строк (правки, undo/redo, фолды). Ключ кэша мапы visible→real
    /// в виджете (4.5/D9.5). Закладки мапу не меняют и версию не трогают.
    view_version: u64,
    /// Первая строка последней правки (до-координаты) — для частичной
    /// инвалидации совпадений поиска (D13); None — правок не было
    /// (undo/redo строки правок не знают — панель делает полный
    /// пересчёт). Забирается (`take`) потребителем.
    last_edit_line: Option<usize>,
    /// Фоновый проход раскраски запущен (один ход на открытие:
    /// повторного перезапуска нет — правки фильтруются bg_valid_upto,
    /// смена языка блокирует слияние по имени синтаксиса; ходка по
    /// старому снимку может доделаться вхолостую — осознанный MVP).
    pub(crate) bg_walk_started: bool,
    /// Первая строка правок с момента старта фоновой ходки
    /// (usize::MAX — правок не было): фильтр устаревших чекпоинтов и
    /// спанов при слиянии (undo/redo — 0: строки правок неизвестны).
    pub(crate) bg_valid_upto: usize,
    /// Сохранённые метки воркспейса (.settings) применены к вкладке
    /// (гидратация один раз; сбрасывается при загрузке настроек
    /// другого воркспейса). Повторная установка поверх ручных правок
    /// не делается — повторное открытие файла = новая вкладка.
    pub(crate) marks_hydrated: bool,
}

impl BufferTab {
    /// Версия вида (ключ кэша мапы folding, D9.5).
    pub fn view_version(&self) -> u64 {
        self.view_version
    }

    fn bump_view(&mut self) {
        self.view_version += 1;
    }

    /// Главная каретка (последний курсор).
    pub fn primary(&self) -> &Selection {
        self.cursors.last().expect("at least one cursor")
    }

    fn primary_mut(&mut self) -> &mut Selection {
        self.cursors.last_mut().expect("at least one cursor")
    }

    // ---- Конструирование, заголовок, сохранение ----

    /// Untitled-вкладка: пустой документ UTF-8/LF (дефолт FileFormat).
    pub fn untitled() -> Self {
        BufferTab {
            path: None,
            doc: Document::new(),
            history: History::new(),
            cursors: vec![Selection::caret(0)],
            goal_col: None,
            dirty: false,
            scroll_y: 0.0,
            max_seen_cols: 0,
            language: TabLanguage::default(),
            highlight: HighlightEngine::new(),
            view_version: 0,
            last_edit_line: None,
            bg_walk_started: false,
            bg_valid_upto: usize::MAX,
            marks_hydrated: false,
        }
    }

    /// Открыть файл (кодировка/переводы строк — детект editor-core 2.5).
    /// Язык: файл больше 20 МБ помечается `over_threshold` (подсветка
    /// выключена; включение — согласие пользователя через диалог,
    /// D12.4); детект — только если набор синтаксисов уже прогрет
    /// (иначе — прогрев после первого кадра, D12.1).
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let doc = Document::load_file_detected(&path)?;
        let mut language = TabLanguage::default();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size > crate::language::HIGHLIGHT_MAX_BYTES {
            language.over_threshold = true;
        }
        let mut tab = BufferTab {
            path: Some(path),
            doc,
            history: History::new(),
            cursors: vec![Selection::caret(0)],
            goal_col: None,
            dirty: false,
            scroll_y: 0.0,
            max_seen_cols: 0,
            language,
            highlight: HighlightEngine::new(),
            view_version: 0,
            last_edit_line: None,
            bg_walk_started: false,
            bg_valid_upto: usize::MAX,
            marks_hydrated: false,
        };
        tab.detect_language_if_warm();
        tab.rescan_line_width();
        Ok(tab)
    }

    /// Точный максимум ширины строк (char-колонки, без терминаторов).
    /// Вызывается на open: один проход O(n) — та же цена, что сбор
    /// LineMeta при загрузке; правки НЕ пересканируют (ракетка рендера
    /// ловит рост, лёгкий перебор ширины до reopen — компромисс).
    fn rescan_line_width(&mut self) {
        self.max_seen_cols = (0..self.doc.len_lines())
            .map(|l| self.doc.line_content(l).chars().count())
            .max()
            .unwrap_or(0);
    }

    /// Ракетка ширины рендера: строка шире виденного — запомнить,
    /// `true` — контент вырос (вызов перерисует контейнер скролла).
    pub(crate) fn note_line_width(&mut self, cols: usize) -> bool {
        if cols > self.max_seen_cols {
            self.max_seen_cols = cols;
            true
        } else {
            false
        }
    }

    /// Ширина контента для горизонтального скролла (char-колонки).
    pub fn max_seen_cols(&self) -> usize {
        self.max_seen_cols
    }

    /// Детект языка без инициализации набора (открытие файла — бюджет
    /// D11.2 не платит за syntect-инициализацию).
    fn detect_language_if_warm(&mut self) {
        if self.language.disabled() {
            return;
        }
        if let Some(found) = crate::language::detect_if_warm(
            self.path.as_deref(),
            Some(&first_line_snippet(&self.doc)),
        ) {
            self.language.auto = Some(found);
        }
        self.sync_highlight_syntax();
    }

    /// Детект языка с инициализацией набора (прогрев после первого
    /// кадра, ручной сброс в Auto, save-as). Меняет грамматику движка.
    pub fn refresh_language(&mut self) {
        if self.language.disabled() {
            return;
        }
        self.language.auto = crate::language::detect_language(
            self.path.as_deref(),
            Some(&first_line_snippet(&self.doc)),
        );
        self.sync_highlight_syntax();
    }

    /// Разрешение подсветки сверх порога 20 МБ (согласие пользователя
    /// в диалоге, D12.4): включает и детект — движок красит файл
    /// любого размера (догон с бюджетом держит кадры).
    pub fn allow_highlight_despite_threshold(&mut self) {
        if !self.language.over_threshold || self.language.threshold_allowed {
            return;
        }
        self.language.threshold_allowed = true;
        self.refresh_language();
    }

    /// Ручной выбор языка (оверлей 5.1): грамматика None — Plain Text.
    /// Синхронизирует движок (сброс кэшей при смене — внутри движка).
    pub fn set_language(&mut self, syntax: Option<&'static SyntaxReference>) {
        self.language.manual = match syntax {
            None => LanguageOverride::PlainText,
            Some(s) => LanguageOverride::Syntax(s),
        };
        self.sync_highlight_syntax();
    }

    /// Ручной сброс в авто-детект.
    pub fn set_language_auto(&mut self) {
        self.language.manual = LanguageOverride::Auto;
        self.sync_highlight_syntax();
    }

    /// Метка языка статус-бара.
    pub fn language_label(&self) -> &'static str {
        self.language.label()
    }

    /// Вкладка — CSV/TSV (липкая шапка заголовка, D12.5): активная
    /// грамматика «CSV» (одна на .csv/.tsv — заглушка D12.4) либо путь
    /// с таким расширением. Расширение покрывает порог-отключение
    /// подсветки (>20 МБ, отказ): шапка липнет и без цветовых спанов
    /// (зафиксировано в D12.5).
    pub fn is_csv(&self) -> bool {
        if self.highlight.syntax_name() == Some("CSV") {
            return true;
        }
        self.path
            .as_deref()
            .and_then(|p| p.extension().and_then(|e| e.to_str()))
            .is_some_and(|e| e.eq_ignore_ascii_case("csv") || e.eq_ignore_ascii_case("tsv"))
    }

    /// Синхронизировать грамматику движка с состоянием языка.
    fn sync_highlight_syntax(&mut self) {
        let syntax = self.language.effective();
        if self.highlight.syntax_name() != syntax.map(|s| s.name.as_str()) {
            self.highlight.set_syntax(syntax);
        }
    }

    /// Заголовок вкладки: имя файла или «Untitled N» (номер задаёт модель).
    pub fn title(&self, untitled_no: u32) -> String {
        match &self.path {
            Some(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.to_string_lossy().into_owned()),
            None => format!("Untitled {untitled_no}"),
        }
    }

    /// Сохранить в путь; успешное сохранение сбрасывает dirty и
    /// запоминает путь (для save-as и первого сохранения untitled).
    /// После save-as расширение могло измениться — пере-детект (5.1).
    pub fn save_to(&mut self, path: impl Into<PathBuf>) -> io::Result<()> {
        let path = path.into();
        self.doc.save_file(&path)?;
        self.path = Some(path);
        self.dirty = false;
        self.refresh_language();
        Ok(())
    }

    /// Сохранить по собственному пути (untitled — ошибка, GUI сначала
    /// показывает save-as диалог).
    pub fn save(&mut self) -> io::Result<()> {
        let path = self.path.clone().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "untitled tab has no path")
        })?;
        self.doc.save_file(&path)?;
        self.dirty = false;
        Ok(())
    }

    // ---- Редактирование (через editor-core, все курсоры сразу) ----

    /// Ввести текст во все курсоры. Любая применённая правка делает
    /// вкладку dirty и регистрируется в History.
    pub fn insert_text(&mut self, text: &str) {
        let tx = insert_text(&self.cursors, text);
        self.apply_tx(tx);
    }

    /// Backspace: выделение удаляется целиком, иначе символ перед
    /// кареткой (каретка в 0 — no-op для этого курсора).
    pub fn backspace(&mut self) {
        let tx = backspace(self.doc.len_chars(), &self.cursors);
        self.apply_tx(tx);
    }

    /// Forward delete (каретка в конце — no-op).
    pub fn delete_forward(&mut self) {
        let tx = forward_delete(self.doc.len_chars(), &self.cursors);
        self.apply_tx(tx);
    }

    /// Текст всех выделений для копирования: диапазоны в порядке
    /// документа, несколько курсоров — через '\n' (стиль VS Code).
    /// None — выделяемого текста нет.
    pub fn copy_text(&self) -> Option<String> {
        let mut ranges: Vec<(usize, usize)> = self
            .cursors
            .iter()
            .filter(|s| !s.is_caret())
            .map(|s| (s.start, s.end))
            .collect();
        if ranges.is_empty() {
            return None;
        }
        ranges.sort_unstable();
        Some(
            ranges
                .iter()
                .map(|&(a, b)| self.doc.slice_str(a, b))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// Cut: тот же текст, что copy, затем удаление всех выделений
    /// (каретки не трогаем) одной транзакцией — один шаг undo.
    pub fn cut_text(&mut self) -> Option<String> {
        let text = self.copy_text()?;
        let tx = editor_core::Transaction::from_edits(
            self.cursors
                .iter()
                .filter(|s| !s.is_caret())
                .map(|s| editor_core::Edit::delete(s.start, s.end))
                .collect(),
        );
        self.apply_tx(tx);
        Some(text)
    }

    /// Paste: вставка во все курсоры (как ввод с клавиатуры: выделение
    /// заменяется, каретки смещаются за вставленный текст).
    pub fn paste_text(&mut self, text: &str) {
        self.insert_text(text);
    }

    /// Заменить ВЕСЬ текст документа одной транзакцией (форматтеры
    /// JSON): один шаг undo, каретка в начало. Одинаковый текст —
    /// no-op (dirty не ставится).
    pub fn replace_all_text(&mut self, text: &str) {
        if self.doc.text() == text {
            return;
        }
        let tx = editor_core::Transaction::from_edits(vec![editor_core::Edit {
            start: 0,
            end: self.doc.len_chars(),
            text: text.to_string(),
        }]);
        self.apply_tx(tx);
        self.cursors = vec![Selection::caret(0)];
        self.goal_col = None;
    }

    pub(crate) fn apply_tx(&mut self, tx: editor_core::Transaction) {
        if tx.is_empty() {
            return;
        }
        // Курсоры смещаются правкой в пост-координаты ДО применения —
        // позиции tx до-транзакционные. Единая политика виджета (D2.2
        // + D9.3): после ЛЮБОГО редактирования курсоры схлопываются в
        // каретки в головах; дубликаты (совпавшие после отброшенных
        // пересечений) удаляются — остаётся позднейший (он же главный).
        let mapped: Vec<Selection> =
            self.cursors.iter().map(|s| tx.map_selection(s)).collect();
        self.cursors = dedup_carets(mapped);
        self.goal_col = None;
        // Подсветка (D12.1): правки от первой затронутой строки (в
        // до-координатах) до конца окна/первого валидного стека —
        // внутри движка; строки выше правки не пересчитываются.
        let first_edit_line = tx
            .edits()
            .iter()
            .map(|e| e.start)
            .min()
            .map(|p| self.doc.char_to_line(p))
            .unwrap_or(0);
        self.history.apply(&mut self.doc, &tx);
        self.highlight.invalidate_from(first_edit_line);
        self.dirty = true;
        self.last_edit_line = Some(first_edit_line);
        self.bg_valid_upto = self.bg_valid_upto.min(first_edit_line);
        self.bump_view();
    }

    /// Первая строка последней правки (для частичной инвалидации
    /// совпадений поиска, D13); забирает значение.
    pub fn take_last_edit_line(&mut self) -> Option<usize> {
        self.last_edit_line.take()
    }

    /// Применить правки поиска/замены (6.2): список замен
    /// `(start, end, text)` в до-координатах — ОДНОЙ составной
    /// транзакцией (undo одним шагом, не группа последовательного
    /// ввода — D2.4). Возвращает число применённых правок.
    pub fn apply_search_edits(&mut self, edits: Vec<(usize, usize, String)>) -> usize {
        let tx = editor_core::Transaction::from_edits(
            edits
                .into_iter()
                .map(|(start, end, text)| editor_core::Edit { start, end, text })
                .collect(),
        );
        let n = tx.edits().len();
        self.apply_tx(tx);
        n
    }

    /// Undo; курсоры клампятся в границы документа (точный маппинг
    /// сквозь undo-группу не нужен для MVP — D9.3). Подсветка: строки
    /// правок неизвестны — полный сброс кэшей (ленивый переразбор,
    /// D12.1).
    pub fn undo(&mut self) -> bool {
        let done = self.history.undo(&mut self.doc);
        if done {
            self.clamp_cursors();
            self.highlight.reset_cache();
            self.bg_valid_upto = 0;
            self.bump_view();
        }
        done
    }

    /// Redo; симметричен undo. Dirty не сбрасывается (нет baseline
    /// «сохранённого состояния» — D8.2).
    pub fn redo(&mut self) -> bool {
        let done = self.history.redo(&mut self.doc);
        if done {
            self.clamp_cursors();
            self.highlight.reset_cache();
            self.bg_valid_upto = 0;
            self.bump_view();
        }
        done
    }

    fn clamp_cursors(&mut self) {
        let len = self.doc.len_chars();
        for c in &mut self.cursors {
            let head = c.head().min(len);
            *c = Selection::caret(head);
        }
    }

    // ---- Навигация (4.2/4.3, GPUI-free) ----
    //
    // `extend: true` — расширение выделения ТОЛЬКО главной каретки
    // (якорь неподвижен, голова движется — D9.3); false — движение
    // ВСЕХ курсоров, выделения сбрасываются в каретки.

    pub fn move_left(&mut self, extend: bool) {
        self.goal_col = None;
        if extend {
            let head = self.primary().head();
            self.extend_primary(head.saturating_sub(1));
        } else {
            for c in &mut self.cursors {
                *c = Selection::caret(c.head().saturating_sub(1));
            }
        }
    }

    pub fn move_right(&mut self, extend: bool) {
        self.goal_col = None;
        if extend {
            let head = self.primary().head();
            self.extend_primary((head + 1).min(self.doc.len_chars()));
        } else {
            let len = self.doc.len_chars();
            for c in &mut self.cursors {
                *c = Selection::caret((c.head() + 1).min(len));
            }
        }
    }

    pub fn move_up(&mut self, extend: bool) {
        self.move_vertically(-1, extend);
    }

    pub fn move_down(&mut self, extend: bool) {
        self.move_vertically(1, extend);
    }

    /// PgUp/PgDn: `lines` — высота страницы в строках (считает виджет,
    /// D9.2); движение с сохранением goal-столбца, clamp по границам.
    pub fn move_page(&mut self, pages: isize, lines: usize, extend: bool) {
        let delta = pages * lines.max(1) as isize;
        if delta != 0 {
            self.move_vertically(delta, extend);
        }
    }

    pub fn move_home(&mut self, extend: bool) {
        self.goal_col = None;
        if extend {
            let head = self.primary().head();
            let line = self.doc.char_to_line(head);
            self.extend_primary(self.doc.line_to_char(line));
        } else {
            let starts: Vec<usize> = self
                .cursors
                .iter()
                .map(|c| {
                    let line = self.doc.char_to_line(c.head());
                    self.doc.line_to_char(line)
                })
                .collect();
            for (c, start) in self.cursors.iter_mut().zip(starts) {
                *c = Selection::caret(start);
            }
        }
    }

    pub fn move_end(&mut self, extend: bool) {
        self.goal_col = None;
        if extend {
            let head = self.primary().head();
            let line = self.doc.char_to_line(head);
            let end = self.doc.line_to_char(line) + self.line_len(line);
            self.extend_primary(end);
        } else {
            let ends: Vec<usize> = self
                .cursors
                .iter()
                .map(|c| {
                    let line = self.doc.char_to_line(c.head());
                    self.doc.line_to_char(line) + self.line_len(line)
                })
                .collect();
            for (c, end) in self.cursors.iter_mut().zip(ends) {
                *c = Selection::caret(end);
            }
        }
    }

    /// Клик мышью: ЕДИНСТВЕННАЯ каретка в (`line`, `col`) — все
    /// дополнительные курсоры сбрасываются (clamp по длине строки).
    pub fn set_caret_at(&mut self, line: usize, col: usize) {
        self.goal_col = None;
        let pos = self.clamp_pos(line, col);
        self.cursors = vec![Selection::caret(pos)];
    }

    /// Select All (Cmd+A / контекстное меню): единственное выделение
    /// всего документа, голова в конец.
    pub fn select_all(&mut self) {
        let len = self.doc.len_chars();
        self.cursors = vec![Selection::range(0, len)];
        self.goal_col = None;
    }

    /// Правый клик (контекстное меню): позиция клика внутри выделения —
    /// выделение сохраняется (Copy скопирует его); иначе каретка в
    /// позицию клика. Возвращает строку под кликом (для закладки).
    pub fn right_click_anchor(&mut self, line: usize, col: usize) -> usize {
        let pos = self.clamp_pos(line, col);
        let inside_selection = self
            .cursors
            .iter()
            .any(|s| !s.is_caret() && s.start <= pos && pos <= s.end);
        if !inside_selection {
            self.set_caret_at(line, col);
        }
        self.cursor_line()
    }

    /// Расширение выделения до позиции клика/драга: якорь главной
    /// каретки неподвижен, остальные курсоры сохраняются.
    pub fn select_to(&mut self, line: usize, col: usize) {
        self.goal_col = None;
        let pos = self.clamp_pos(line, col);
        self.extend_primary(pos);
    }

    // ---- Закладки (4.4; хранятся в LineMeta, переживают undo/redo) ----

    /// Cmd+F2: поставить/снять закладку на строке главной каретки.
    pub fn toggle_bookmark_on_primary(&mut self) {
        let line = self.cursor_line();
        self.doc.line_meta_mut().toggle_bookmark(line);
    }

    /// F2: следующая закладка по кругу от строки главной каретки.
    /// Каретка (единственная) переходит в начало строки закладки;
    /// `true` — переход случился.
    pub fn goto_next_bookmark(&mut self) -> bool {
        let from = self.cursor_line();
        self.doc
            .line_meta()
            .next_bookmark(from)
            .map(|line| {
                self.set_caret_at(line, 0);
                true
            })
            .unwrap_or(false)
    }

    /// Shift+F2: предыдущая закладка по кругу.
    pub fn goto_prev_bookmark(&mut self) -> bool {
        let from = self.cursor_line();
        self.doc
            .line_meta()
            .prev_bookmark(from)
            .map(|line| {
                self.set_caret_at(line, 0);
                true
            })
            .unwrap_or(false)
    }

    /// Строки с закладками активного файла (для панели, 4.4).
    pub fn bookmarked_lines(&self) -> Vec<usize> {
        self.doc.line_meta().bookmarked_lines()
    }

    /// Переключить закладку на строке (клик по маркеру в гуттере).
    pub fn toggle_bookmark_at(&mut self, line: usize) {
        self.doc.line_meta_mut().toggle_bookmark(line);
    }

    // ---- Операции со строками (4.6, D9.6) ----

    /// Строки, пересекаемые выделениями курсоров (не пустыми);
    /// `None` — выделений нет, операция над всем файлом.
    /// Выделение, заканчивающееся в колонке 0, строку не захватывает.
    fn selection_lines(&self) -> Option<(usize, usize)> {
        let mut scope: Option<(usize, usize)> = None;
        for sel in &self.cursors {
            if sel.is_caret() {
                continue;
            }
            let first = self.doc.char_to_line(sel.start);
            let mut last = self.doc.char_to_line(sel.end);
            if last > first && self.doc.line_to_char(last) == sel.end {
                last -= 1; // конец в колонке 0 строки `last`
            }
            scope = Some(match scope {
                None => (first, last),
                Some((f, l)) => (f.min(first), l.max(last)),
            });
        }
        scope
    }

    fn apply_block_op(
        &mut self,
        op: impl Fn(&editor_core::Document, usize, usize) -> editor_core::Transaction,
    ) {
        let (first, mut last) = self.selection_lines().unwrap_or_else(|| {
            (0, self.doc.len_lines().saturating_sub(1))
        });
        // Whole-file: фантомная хвостовая пустая строка ropey (после
        // завершающего \n) не участвует в операциях.
        if last > first && self.doc.line_str(last).is_empty() {
            last -= 1;
        }
        let tx = op(&self.doc, first, last);
        self.apply_tx(tx);
    }

    /// Сортировка строк выделения (или всего файла).
    pub fn sort_lines(&mut self, ascending: bool, ignore_case: bool) {
        self.apply_block_op(|doc, f, l| {
            editor_core::sort_lines(doc, f, l, ascending, ignore_case)
        });
    }

    /// Удаление дубликатов (порядок первых вхождений).
    pub fn dedupe_lines(&mut self) {
        self.apply_block_op(editor_core::dedupe_lines);
    }

    /// Удаление пустых строк.
    pub fn remove_empty_lines(&mut self) {
        self.apply_block_op(editor_core::remove_empty_lines);
    }

    /// Регистр: на выделениях (каждое — отдельный диапазон) или всём
    /// файле, если непустых выделений нет.
    pub fn change_case(&mut self, mode: editor_core::CaseMode) {
        let ranges: Vec<(usize, usize)> = self
            .cursors
            .iter()
            .filter(|s| !s.is_caret())
            .map(|s| (s.start, s.end))
            .collect();
        let tx = if ranges.is_empty() {
            editor_core::change_case(&self.doc, &[(0, self.doc.len_chars())], mode)
        } else {
            editor_core::change_case(&self.doc, &ranges, mode)
        };
        self.apply_tx(tx);
    }

    // ---- Свёртывание (4.5, D9.5) ----

    /// Toggle фолда на строке-заголовке (клик по маркеру). Не-заголовок
    /// — no-op.
    pub fn toggle_fold(&mut self, line: usize) {
        if crate::folding::is_foldable(&self.doc, line) {
            self.doc.line_meta_mut().toggle_folded(line);
            self.bump_view();
        }
    }

    /// Применить сохранённое состояние строк воркспейса
    /// (.settings/workspace.toml): закладки и фолды; строки вне
    /// диапазона документа (файл изменился извне) — игнорируются.
    /// Стыкуется с view-версией — фолд-мапа пересоберётся.
    pub fn apply_line_marks(&mut self, bookmarks: &[usize], folds: &[usize]) {
        let len = self.doc.len_lines();
        {
            let meta = self.doc.line_meta_mut();
            for &l in bookmarks.iter().filter(|l| **l < len) {
                meta.set_bookmark(l);
            }
            for &l in folds.iter().filter(|l| **l < len) {
                meta.set_folded(l, true);
            }
        }
        if !bookmarks.is_empty() || !folds.is_empty() {
            self.bump_view();
        }
    }

    /// Свернуть все блоки (заголовки по отступам).
    pub fn fold_all(&mut self) {
        let headers: Vec<usize> = (0..self.doc.len_lines())
            .filter(|l| crate::folding::is_foldable(&self.doc, *l))
            .collect();
        {
            let meta = self.doc.line_meta_mut();
            for l in headers {
                meta.set_folded(l, true);
            }
        }
        self.bump_view();
    }

    /// Развернуть всё.
    pub fn unfold_all(&mut self) {
        self.doc.line_meta_mut().folded_lines().iter().for_each(|l| {
            self.doc.line_meta_mut().set_folded(*l, false);
        });
        self.bump_view();
    }

    /// Строка видима? (не внутри свёрнутого тела)
    #[allow(dead_code)] // часть API модели (проверки видимости из виджета)
    pub fn is_line_visible(&self, line: usize) -> bool {
        crate::folding::enclosing_fold_header(&self.doc, line).is_none()
    }

    /// Ближайшая ВИДИМАЯ строка: скрытая строка → заголовок её фолда.
    /// Для движения ВВЕРХ и перехода к строке (Cmd+G).
    pub fn visible_line_or_header(&self, line: usize) -> usize {
        crate::folding::enclosing_fold_header(&self.doc, line).unwrap_or(line)
    }

    /// Первая видимая строка НАЧИНАЯ С `line` (для движения ВНИЗ:
    /// тело свёрнутого блока под текущей строкой перепрыгивается).
    pub fn first_visible_at_or_after(&self, line: usize) -> usize {
        let mut l = line;
        for _ in 0..64 {
            let Some(header) = crate::folding::enclosing_fold_header(&self.doc, l) else {
                return l;
            };
            l = crate::folding::fold_end(&self.doc, header) + 1;
        }
        self.doc.len_lines().saturating_sub(1)
    }

    // ---- Мульти-курсор (4.3, D9.3) ----

    /// Двойной клик: выделить слово под позицией (нет слова — каретка).
    pub fn select_word_at(&mut self, line: usize, col: usize) {
        self.goal_col = None;
        let pos = self.clamp_pos(line, col);
        self.cursors = match self.word_at(pos) {
            Some((s, e)) => vec![Selection::range(s, e)],
            None => vec![Selection::caret(pos)],
        };
    }

    /// Тройной клик: выделить строку целиком (с терминатором).
    pub fn select_line_at(&mut self, line: usize) {
        self.goal_col = None;
        let line = line.min(self.doc.len_lines().saturating_sub(1));
        let start = self.doc.line_to_char(line);
        let end = if line + 1 < self.doc.len_lines() {
            self.doc.line_to_char(line + 1)
        } else {
            self.doc.len_chars()
        };
        self.cursors = vec![Selection::range(start, end)];
    }

    /// Заменить весь набор курсоров (якорь/головы уже в char-координатах).
    #[allow(dead_code)] // установка набора кареток тестами/GUI напрямую
    pub fn set_cursors(&mut self, cursors: Vec<Selection>) {
        self.goal_col = None;
        if cursors.is_empty() {
            return;
        }
        self.cursors = cursors;
    }

    /// Cmd+клик: добавить каретку в позицию. Повторный Cmd+клик в ту же
    /// точку снимает курсор (toggle, как в VS Code); совпадение с
    /// существующей кареткой без Cmd — обычный клик, сюда не попадает.
    pub fn add_cursor_at(&mut self, line: usize, col: usize) {
        self.goal_col = None;
        let pos = self.clamp_pos(line, col);
        let caret = Selection::caret(pos);
        if let Some(i) = self.cursors.iter().position(|c| *c == caret) {
            if self.cursors.len() > 1 {
                self.cursors.remove(i);
            }
            return;
        }
        self.cursors.push(caret);
    }

    /// Column selection (Alt/Cmd+drag): прямоугольник между якорем и
    /// головой → каретки/выделения в начале каждой строки блока.
    /// Короткие строки дают каретку в их конце (VS Code-семантика).
    pub fn set_column_block(&mut self, anchor: (usize, usize), head: (usize, usize)) {
        self.goal_col = None;
        self.cursors = column_block(&self.doc, anchor, head);
    }

    /// Cmd+D: первый вызов — выделить слово под главной кареткой;
    /// следующий — добавить следующее вхождение того же текста
    /// (циклически, без пересечений с существующими курсорами).
    /// `false` — добавлять нечего.
    pub fn add_next_word_match(&mut self) -> bool {
        let primary = *self.primary();
        if primary.is_caret() {
            if let Some((s, e)) = self.word_at(primary.head()) {
                *self.primary_mut() = Selection::range(s, e);
                return true;
            }
            return false;
        }
        let needle = self.doc.slice_str(primary.start, primary.end);
        if needle.is_empty() {
            return false;
        }
        let len = self.doc.len_chars();
        // После конца главного выделения, затем циклически — с начала
        // и до его начала.
        let found = self
            .find_from(&needle, primary.end, len)
            .or_else(|| self.find_from(&needle, 0, primary.start));
        if let Some((s, e)) = found {
            let overlaps = self
                .cursors
                .iter()
                .any(|sel| s < sel.end && e > sel.start);
            if !overlaps {
                self.cursors.push(Selection::range(s, e));
                return true;
            }
        }
        false
    }

    // ---- Внутренние хелперы ----

    /// Слово под позицией (границы: буква/цифра/подчёркивание).
    /// Каретка сразу после слова — берём слово слева.
    fn word_at(&self, pos: usize) -> Option<(usize, usize)> {
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let char_at = |p: usize| -> Option<char> {
            if p < self.doc.len_chars() {
                self.doc.slice_str(p, p + 1).chars().next()
            } else {
                None
            }
        };
        if char_at(pos).is_some_and(is_word) {
            let mut s = pos;
            while s > 0 && char_at(s - 1).is_some_and(is_word) {
                s -= 1;
            }
            let mut e = pos;
            let len = self.doc.len_chars();
            while e < len && char_at(e).is_some_and(is_word) {
                e += 1;
            }
            return Some((s, e));
        }
        if pos > 0 && char_at(pos - 1).is_some_and(is_word) {
            let mut s = pos;
            while s > 0 && char_at(s - 1).is_some_and(is_word) {
                s -= 1;
            }
            return Some((s, pos));
        }
        None
    }

    /// Первое вхождение `needle` в char-диапазоне `[from, to)`;
    /// `to < from` → None.
    fn find_from(&self, needle: &str, from: usize, to: usize) -> Option<(usize, usize)> {
        if to <= from {
            return None;
        }
        let hay = self.doc.slice_str(from, to);
        let byte_idx = hay.find(needle)?;
        let start = from + hay[..byte_idx].chars().count();
        Some((start, start + needle.chars().count()))
    }

    /// Поставить голову главной каретки, сохранив якорь.
    fn extend_primary(&mut self, pos: usize) {
        let anchor = self.primary().anchor();
        *self.primary_mut() = Selection::range(anchor, pos);
    }

    fn clamp_pos(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.doc.len_lines().saturating_sub(1));
        let col = col.min(self.line_len(line));
        self.doc.line_to_char(line) + col
    }

    fn move_vertically(&mut self, delta: isize, extend: bool) {
        let lines = self.doc.len_lines();
        // Вертикальное движение не заходит в свёрнутые тела (4.5):
        // вниз — первая видимая от цели, вверх — заголовок блока.
        let target_line = |tab: &BufferTab, line: usize| {
            let clamped = if delta < 0 {
                line.saturating_sub(delta.unsigned_abs())
            } else {
                (line + delta.unsigned_abs()).min(lines.saturating_sub(1))
            };
            if delta < 0 {
                tab.visible_line_or_header(clamped)
            } else {
                tab.first_visible_at_or_after(clamped)
            }
        };
        if extend {
            // Расширяется только главная каретка (D9.3), с goal-столбцом.
            let head = self.primary().head();
            let line = self.doc.char_to_line(head);
            let col = head - self.doc.line_to_char(line);
            let goal = *self.goal_col.get_or_insert(col);
            let new_line = target_line(self, line);
            self.extend_primary(self.doc.line_to_char(new_line) + goal.min(self.line_len(new_line)));
            return;
        }
        if self.cursors.len() == 1 {
            // Одиночная каретка: goal column работает (D9.2).
            let head = self.cursors[0].head();
            let line = self.doc.char_to_line(head);
            let col = head - self.doc.line_to_char(line);
            let goal = *self.goal_col.get_or_insert(col);
            let new_line = target_line(self, line);
            let new_col = goal.min(self.line_len(new_line));
            self.cursors[0] = Selection::caret(self.doc.line_to_char(new_line) + new_col);
        } else {
            // Мульти-курсор: goal-столбец у каждого свой — не храним,
            // клампим по столбцу текущей строки (D9.3).
            let moved: Vec<Selection> = self
                .cursors
                .iter()
                .map(|c| {
                    let head = c.head();
                    let line = self.doc.char_to_line(head);
                    let col = head - self.doc.line_to_char(line);
                    let new_line = target_line(self, line);
                    Selection::caret(
                        self.doc.line_to_char(new_line) + col.min(self.line_len(new_line)),
                    )
                })
                .collect();
            self.cursors = moved;
        }
    }

    /// Строка ГЛАВНОЙ каретки (0-based).
    pub fn cursor_line(&self) -> usize {
        self.doc.char_to_line(self.primary().head())
    }

    /// Столбец ГЛАВНОЙ каретки (0-based, char-единицы).
    pub fn cursor_col(&self) -> usize {
        let line = self.cursor_line();
        self.primary().head() - self.doc.line_to_char(line)
    }

    /// Длина строки без терминатора.
    fn line_len(&self, line: usize) -> usize {
        self.doc.line_content(line).chars().count()
    }
}

/// Первые ~1 КБ первой строки документа — эвристика содержимого для
/// детекта языка (5.1; find_syntax_by_first_line хватает префикса).
fn first_line_snippet(doc: &Document) -> String {
    let mut s = doc.line_content(0);
    s.truncate(s.len().min(1024));
    s
}

/// После редактирования: все курсоры → каретки в головах; дубликаты
/// удаляются с сохранением ПОЗДНЕГО вхождения (он же главный, D9.3).
fn dedup_carets(sels: Vec<Selection>) -> Vec<Selection> {
    let carets: Vec<Selection> = sels.iter().map(|s| Selection::caret(s.head())).collect();
    let mut out: Vec<Selection> = Vec::with_capacity(carets.len());
    for (i, c) in carets.iter().enumerate() {
        if carets[i + 1..].contains(c) {
            continue; // такой же курсор позже — этот дропаем
        }
        if out.last() != Some(c) {
            out.push(*c);
        }
    }
    if out.is_empty() {
        out.push(Selection::caret(0));
    }
    out
}

/// Прямоугольник (якорь, голова) → курсоры на каждую строку блока:
/// выделение [min_col..max_col), кламп по длине строки; строка короче
/// min_col → каретка в её конце.
fn column_block(doc: &Document, anchor: (usize, usize), head: (usize, usize)) -> Vec<Selection> {
    let (l0, l1) = if anchor.0 <= head.0 { (anchor.0, head.0) } else { (head.0, anchor.0) };
    let (c0, c1) = if anchor.1 <= head.1 { (anchor.1, head.1) } else { (head.1, anchor.1) };
    let last_line = doc.len_lines().saturating_sub(1);
    let mut out = Vec::with_capacity(l1 - l0 + 1);
    for line in l0..=l1.min(last_line) {
        let len = doc.line_content(line).chars().count();
        let start = doc.line_to_char(line);
        let s = c0.min(len);
        let e = c1.min(len);
        out.push(if s == e {
            Selection::caret(start + s)
        } else {
            Selection::range(start + s, start + e)
        });
    }
    if out.is_empty() {
        out.push(Selection::caret(0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_untitled_without_path_is_error() {
        let mut t = BufferTab::untitled();
        let err = t.save().unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    /// Буфер обмена: copy без выделения — None; copy возвращает текст
    /// диапазона; cut удаляет выделение одним undo-шагом; paste
    /// вставляет во все курсоры.
    #[test]
    fn clipboard_copy_cut_paste() {
        let mut t = BufferTab::from_text("hello world");
        // Нет выделения — копировать нечего.
        assert_eq!(t.copy_text(), None);

        // Copy диапазона: текст без изменения документа.
        t.set_cursors(vec![Selection::range(6, 11)]);
        assert_eq!(t.copy_text().as_deref(), Some("world"));
        assert_eq!(t.doc.text(), "hello world");

        // Cut: тот же текст, документ изменён, один undo возвращает.
        assert_eq!(t.cut_text().as_deref(), Some("world"));
        assert_eq!(t.doc.text(), "hello ");
        assert!(t.undo());
        assert_eq!(t.doc.text(), "hello world");

        // Paste замены выделения.
        t.set_cursors(vec![Selection::range(0, 5)]);
        t.paste_text("goodbye");
        assert_eq!(t.doc.text(), "goodbye world");

        // Мульти-курсор: выделения в порядке документа через '\n'.
        let mut t = BufferTab::from_text("aaa bbb ccc");
        t.set_cursors(vec![Selection::range(8, 11), Selection::range(0, 3)]);
        assert_eq!(t.copy_text().as_deref(), Some("aaa\nccc"));
    }

    /// Select All: единственное выделение [0..len), голова в конец.
    #[test]
    fn select_all_covers_document() {
        let mut t = BufferTab::from_text("hello\nworld");
        t.set_caret_at(1, 2);
        t.select_all();
        assert_eq!(t.primary(), &Selection::range(0, 11));
        assert!(!t.primary().is_caret());
        // Copy после select all — весь текст (вместе с переносом).
        assert_eq!(t.copy_text().as_deref(), Some("hello\nworld"));
        // Cut очищает документ целиком, undo возвращает.
        assert_eq!(t.cut_text().as_deref(), Some("hello\nworld"));
        assert_eq!(t.doc.text(), "");
        assert!(t.undo());
        assert_eq!(t.doc.text(), "hello\nworld");
    }

    /// Редактирование через editor-core: ввод/undo/redo, каретка
    /// смещается правками (map_selection), документ LF-нормализован.
    #[test]
    fn editing_through_history() {
        let mut t = BufferTab::untitled();
        for ch in "hello".chars() {
            t.insert_text(&ch.to_string());
        }
        assert_eq!(t.doc.text(), "hello");
        assert_eq!(*t.primary(), Selection::caret(5));
        assert!(t.dirty);

        // Каретка в середину, ввод — позиция каретки честно уезжает.
        // Одночные вставки продолжают ту же undo-группу (D2.4), поэтому
        // один undo снимает весь ввод целиком.
        t.set_caret_at(0, 2);
        t.insert_text("X");
        assert_eq!(t.doc.text(), "heXllo");
        assert_eq!(*t.primary(), Selection::caret(3));

        assert!(t.undo());
        assert_eq!(t.doc.text(), "");
        assert!(t.primary().head() <= 5);
        assert!(t.redo());
        assert_eq!(t.doc.text(), "heXllo");
        // Redo закрыл группу: результат можно снова undo (D2.4).
        assert!(t.undo());
        assert_eq!(t.doc.text(), "");

        // Составная вставка — своя группа: «hello», затем «XY» — два undo.
        let mut t = BufferTab::untitled();
        for ch in "hello".chars() {
            t.insert_text(&ch.to_string());
        }
        t.insert_text("XY"); // мульти-символьная — закрывает группу
        assert_eq!(t.doc.text(), "helloXY");
        assert!(t.undo());
        assert_eq!(t.doc.text(), "hello");
        assert!(t.undo());
        assert_eq!(t.doc.text(), "");
        // Dirty не сбрасывается undo (нет baseline, D8.2).
        assert!(t.dirty);
    }

    /// 4.3: ввод на нескольких каретках; после правки курсоры —
    /// каретки за вставленным текстом (D2.2/D9.3).
    #[test]
    fn multi_cursor_typing() {
        let mut t = BufferTab::from_text("one two three");
        t.set_cursors(vec![
            Selection::caret(3),
            Selection::caret(7),
            Selection::caret(13),
        ]);
        t.insert_text(",");
        assert_eq!(t.doc.text(), "one, two, three,");
        assert_eq!(
            t.cursors,
            vec![
                Selection::caret(4),
                Selection::caret(9),
                Selection::caret(16),
            ]
        );
        // Один undo снимает всю правку (одна группа: одиночные вставки).
        assert!(t.undo());
        assert_eq!(t.doc.text(), "one two three");
    }

    /// 4.3: ввод ПОВЕРХ выделений схлопывает их в каретки
    /// (политика виджета поверх map_selection — D2.2/D9.3).
    #[test]
    fn insert_over_selections_collapses_to_carets() {
        let mut t = BufferTab::from_text("alpha beta gamma");
        t.set_cursors(vec![
            Selection::range(0, 5),
            Selection::range(6, 10),
            Selection::range(11, 16),
        ]);
        t.insert_text("X");
        assert_eq!(t.doc.text(), "X X X");
        // Каретки за каждым вставленным X (Bias::Right): 1, 3, 5.
        assert_eq!(
            t.cursors,
            vec![Selection::caret(1), Selection::caret(3), Selection::caret(5)]
        );
    }

    /// 4.3: backspace на нескольких каретках; совпавшие каретки
    /// дедуплицируется (остаётся позднейший).
    #[test]
    fn multi_cursor_backspace_dedup() {
        let mut t = BufferTab::from_text("ab");
        t.set_cursors(vec![Selection::caret(1), Selection::caret(2)]);
        t.backspace(); // каждая каретка удаляет символ перед собой
        assert_eq!(t.doc.text(), "");
        // Обе каретки сошлись в 0 — остаётся одна.
        assert_eq!(t.cursors, vec![Selection::caret(0)]);
    }

    /// 4.3: Cmd+клик — toggle курсора в точке.
    #[test]
    fn add_cursor_at_toggles() {
        let mut t = BufferTab::from_text("one\ntwo\n");
        t.add_cursor_at(1, 0);
        assert_eq!(t.cursors.len(), 2);
        assert_eq!(*t.primary(), Selection::caret(4)); // главный — последний
        t.add_cursor_at(1, 0); // повторный — снимает
        assert_eq!(t.cursors.len(), 1);
        // Последний курсор снимать нельзя — остаётся один.
        t.add_cursor_at(0, 0);
        t.add_cursor_at(0, 0);
        assert_eq!(t.cursors.len(), 1);
    }

    /// 4.3: column selection — позиции по строкам блока, короткие
    /// строки дают каретку в конце. "abcdef\nab\nabcdefgh\n":
    /// line0 старт 0, line1 — 7, line2 — 10.
    #[test]
    fn column_block_positions() {
        let doc = Document::from_text("abcdef\nab\nabcdefgh\n");
        // Блок строк 0..=2, колонки 2..5.
        let sels = column_block(&doc, (0, 2), (2, 5));
        assert_eq!(
            sels,
            vec![
                Selection::range(2, 5),     // "abcdef": [2..5)
                Selection::caret(7 + 2),    // "ab" (len 2): каретка в конце
                Selection::range(10 + 2, 10 + 5), // "abcdefgh": [2..5)
            ]
        );
        // Обратное направление (голова выше якоря) — тот же блок.
        let sels_rev = column_block(&doc, (2, 5), (0, 2));
        assert_eq!(sels_rev, sels);
        // Блок из одной строки.
        let one = column_block(&doc, (1, 0), (1, 2));
        assert_eq!(one, vec![Selection::range(7, 9)]);
    }

    /// 4.3: Cmd+D — первый вызов выделяет слово под кареткой, затем
    /// добавляет следующие вхождения; исчерпание — false.
    #[test]
    fn cmd_d_word_matches() {
        let mut t = BufferTab::from_text("foo bar foo baz foo");
        assert!(t.add_next_word_match()); // "foo" [0..3)
        assert_eq!(*t.primary(), Selection::range(0, 3));
        assert!(t.add_next_word_match()); // [8..11)
        assert_eq!(t.cursors.len(), 2);
        assert_eq!(*t.primary(), Selection::range(8, 11));
        assert!(t.add_next_word_match()); // [16..19)
        assert_eq!(*t.primary(), Selection::range(16, 19));
        // Больше вхождений нет — false, курсоры не меняются.
        assert!(!t.add_next_word_match());
        assert_eq!(t.cursors.len(), 3);
        // Ввод на трёх выделениях заменяет их.
        t.insert_text("XX");
        assert_eq!(t.doc.text(), "XX bar XX baz XX");
    }

    /// 4.3: слово под кареткой в середине и сразу после слова.
    #[test]
    fn word_boundaries() {
        let mut t = BufferTab::from_text("fn main() { let x = foo_bar1; }");
        t.set_caret_at(0, 4); // внутри "main"
        assert!(t.add_next_word_match());
        assert_eq!(*t.primary(), Selection::range(3, 7));
        // Каретка сразу ПОСЛЕ слова (на '(').
        t.set_caret_at(0, 7);
        t.set_cursors(vec![Selection::caret(7)]);
        assert!(t.add_next_word_match());
        assert_eq!(*t.primary(), Selection::range(3, 7));
        // Не слово и не сразу после слова (пробел после '(') — false.
        t.set_cursors(vec![Selection::caret(8)]);
        assert!(!t.add_next_word_match());
    }

    /// 4.3: движение без shift двигает ВСЕ каретки; shift расширяет
    /// только главную.
    #[test]
    fn multi_cursor_movement() {
        let mut t = BufferTab::from_text("abcd\nxy\n");
        t.set_cursors(vec![Selection::caret(1), Selection::caret(7)]);
        t.move_left(false);
        assert_eq!(t.cursors, vec![Selection::caret(0), Selection::caret(6)]);
        t.move_right(false);
        t.move_down(false); // 1 → line1 col1 (6); 7 → хвост (2,0) = 8
        assert_eq!(t.cursors, vec![Selection::caret(6), Selection::caret(8)]);
    }

    /// 4.3: shift-движение с несколькими курсорами расширяет только
    /// главный, прочие неподвижны.
    #[test]
    fn shift_extends_primary_only() {
        let mut t = BufferTab::from_text("abcd\nxy\n");
        t.set_cursors(vec![Selection::caret(1), Selection::caret(6)]);
        t.move_right(true);
        assert_eq!(t.cursors[0], Selection::caret(1));
        assert_eq!(t.cursors[1], Selection::range(6, 7));
    }

    /// 4.4 (обязательный): закладки переживают undo/redo — флаги
    /// привязаны к содержимому строк и синхронизируются в Document::apply.
    #[test]
    fn bookmarks_survive_undo_redo() {
        let mut t = BufferTab::from_text("alpha\nbeta\ngamma\n");
        t.set_caret_at(1, 0);
        t.toggle_bookmark_on_primary();
        assert!(t.doc.line_meta().is_bookmarked(1));
        // Вставка двух строк ВЫШЕ закладки: флаг уезжает за содержимым.
        t.set_caret_at(0, 0);
        t.insert_text("x\ny\n");
        assert_eq!(t.doc.text(), "x\ny\nalpha\nbeta\ngamma\n");
        assert!(t.doc.line_meta().is_bookmarked(3), "bookmark follows line content");
        // Undo возвращает текст И позицию закладки.
        assert!(t.undo());
        assert_eq!(t.doc.text(), "alpha\nbeta\ngamma\n");
        assert!(t.doc.line_meta().is_bookmarked(1));
        assert!(!t.doc.line_meta().is_bookmarked(3));
        // Redo — снова за содержимым.
        assert!(t.redo());
        assert!(t.doc.line_meta().is_bookmarked(3));
        // Повторный undo — снова к строке 1.
        assert!(t.undo());
        assert!(t.doc.line_meta().is_bookmarked(1));
    }

    /// 4.4: F2/Shift+F2 — навигация по закладкам по кругу.
    #[test]
    fn bookmark_navigation_wraps() {
        let mut t = BufferTab::from_text("a\nb\nc\nd\ne\n");
        t.toggle_bookmark_at(1);
        t.toggle_bookmark_at(3);
        assert_eq!(t.bookmarked_lines(), vec![1, 3]);
        // Со строки 0: вперёд → 1, вперёд → 3, вперёд → 1 (по кругу).
        t.set_caret_at(0, 0);
        assert!(t.goto_next_bookmark());
        assert_eq!(t.cursor_line(), 1);
        assert!(t.goto_next_bookmark());
        assert_eq!(t.cursor_line(), 3);
        assert!(t.goto_next_bookmark());
        assert_eq!(t.cursor_line(), 1);
        // Назад: 3, затем 1, затем 3 (по кругу).
        assert!(t.goto_prev_bookmark());
        assert_eq!(t.cursor_line(), 3);
        assert!(t.goto_prev_bookmark());
        assert_eq!(t.cursor_line(), 1);
        assert!(t.goto_prev_bookmark());
        assert_eq!(t.cursor_line(), 3);
        // Навигация ставит каретку в начало строки.
        assert_eq!(t.cursor_col(), 0);
        // Без закладок — false.
        t.toggle_bookmark_at(1);
        t.toggle_bookmark_at(3);
        assert!(!t.goto_next_bookmark());
        assert!(!t.goto_prev_bookmark());
    }

    /// 4.4: toggle на строке главной каретки; клик по маркеру гуттера —
    /// toggle на произвольной строке.
    #[test]
    fn bookmark_toggle_paths() {
        let mut t = BufferTab::from_text("one\ntwo\nthree\n");
        t.set_caret_at(2, 1);
        t.toggle_bookmark_on_primary();
        assert_eq!(t.bookmarked_lines(), vec![2]);
        t.toggle_bookmark_on_primary(); // снять
        assert!(t.bookmarked_lines().is_empty());
        t.toggle_bookmark_at(0); // как клик по маркеру
        assert_eq!(t.bookmarked_lines(), vec![0]);
    }

    /// 4.5: toggle_fold скрывает тело; вертикальное движение не заходит
    /// в свёрнутые тела; fold_all/unfold_all.
    #[test]
    fn folding_model_navigation() {
        let mut t = BufferTab::from_text(
            "fn main() {\n    if a {\n        b();\n    }\n    tail();\n}\ntop();\n",
        );
        // Строки: 0 fn, 1 if, 2 b(), 3 }, 4 tail, 5 }, 6 top, 7 "".
        assert!(t.is_line_visible(2));
        t.toggle_fold(1); // тело if: строка 2
        assert!(t.doc.line_meta().is_folded(1));
        assert!(!t.is_line_visible(2));
        // Движение вниз со строки 1 → заголовок внешнего хвоста (4), не 2.
        t.set_caret_at(1, 0);
        t.move_down(false); // тело if (строка 2) перепрыгивается → 3 "}"
        assert_eq!(t.cursor_line(), 3);
        // Не-заголовок — no-op.
        t.toggle_fold(2);
        assert!(t.doc.line_meta().is_folded(1));
        // Fold all: оба блока.
        t.fold_all();
        assert!(t.doc.line_meta().is_folded(0));
        assert!(t.doc.line_meta().is_folded(1));
        // Unfold all.
        t.unfold_all();
        assert!(!t.doc.line_meta().is_folded(0));
        assert!(!t.doc.line_meta().is_folded(1));
        // Свёрнутое состояние переживает undo правки (флаги в LineMeta).
        t.toggle_fold(0);
        t.set_caret_at(6, 5);
        t.insert_text("!");
        assert!(t.undo());
        assert!(t.doc.line_meta().is_folded(0), "fold survives undo");
    }

    /// 4.6, сценарий «Сортировка выделения»: выделенные строки
    /// перестраиваются, остальной текст не меняется; undo откатывает.
    #[test]
    fn line_ops_sort_selection_scenario() {
        let mut t = BufferTab::from_text("header\npear\napple\nbanana\nfooter");
        // Выделение строк 1..=3: от (1,0) до (3,6) — конец в середине.
        let s = t.doc.line_to_char(1);
        let e = t.doc.line_to_char(3) + 6;
        t.set_caret_at(1, 0);
        t.select_to(3, 6);
        assert_eq!((t.primary().start, t.primary().end), (s, e));
        t.sort_lines(true, false);
        assert_eq!(t.doc.text(), "header\napple\nbanana\npear\nfooter");
        // Undo через History — операция регистрировалась как правка.
        assert!(t.undo());
        assert_eq!(t.doc.text(), "header\npear\napple\nbanana\nfooter");
        // По убыванию, с учётом и без учёта регистра.
        let mut t = BufferTab::from_text("Beta\nalpha\nCharlie\n");
        t.sort_lines(false, false); // весь файл, байтовый порядок по убыванию
        assert_eq!(t.doc.text(), "alpha\nCharlie\nBeta\n");
        let mut t = BufferTab::from_text("Beta\nalpha\nCharlie\n");
        t.sort_lines(true, true); // ignore case
        assert_eq!(t.doc.text(), "alpha\nBeta\nCharlie\n");
        let mut t = BufferTab::from_text("Beta\nalpha\nCharlie\n");
        t.sort_lines(true, false); // байтовый порядок: 'B' < 'a'
        assert_eq!(t.doc.text(), "Beta\nCharlie\nalpha\n");
    }

    /// 4.6: scope строк из выделений — строки, пересекаемые выделением;
    /// конец в колонке 0 строку не захватывает; несколько выделений —
    /// объединение.
    #[test]
    fn line_ops_scope_from_selections() {
        let mut t = BufferTab::from_text("c\nb\na\nz\ny\nx");
        // Выделение на строках 1..=2 (от col1 строки 1 до col1 строки 2).
        t.set_caret_at(1, 1);
        t.select_to(2, 1); // конец в колонке 1 — строка 2 захватывается
        t.sort_lines(true, false);
        assert_eq!(t.doc.text(), "c\na\nb\nz\ny\nx");
        // Конец ровно в колонке 0: строка под концом не трогается.
        let mut t = BufferTab::from_text("c\nb\na\nz");
        t.set_caret_at(1, 0);
        t.select_to(2, 0); // [line1_start..line2_start) — только строка 1
        t.sort_lines(false, false);
        assert_eq!(t.doc.text(), "c\nb\na\nz", "sort of single line — no-op");
        // Два выделения: объединение диапазонов.
        let mut t = BufferTab::from_text("c\nb\na\nz\ny\nx");
        t.set_cursors(vec![
            Selection::range(t.doc.line_to_char(1), t.doc.line_to_char(2)),
            Selection::range(t.doc.line_to_char(4), t.doc.line_to_char(5)),
        ]);
        t.sort_lines(true, false); // строки 1..=4
        assert_eq!(t.doc.text(), "c\na\nb\ny\nz\nx");
    }

    /// 4.6, сценарий «Удаление дубликатов»: по одному экземпляру,
    /// порядок первых вхождений.
    #[test]
    fn line_ops_dedupe_scenario() {
        let mut t = BufferTab::from_text("b\na\nb\nc\na\nb\n");
        t.dedupe_lines();
        assert_eq!(t.doc.text(), "b\na\nc\n");
        assert!(t.undo());
        assert_eq!(t.doc.text(), "b\na\nb\nc\na\nb\n");
        // На выделении — только выделенные строки.
        let mut t = BufferTab::from_text("keep\nb\nb\nb\nkeep");
        t.set_caret_at(1, 0);
        t.select_to(3, 1);
        t.dedupe_lines();
        assert_eq!(t.doc.text(), "keep\nb\nkeep");
    }

    /// 4.6: удаление пустых строк (выделение/весь файл).
    #[test]
    fn line_ops_remove_empty() {
        let mut t = BufferTab::from_text("a\n\nb\n\n\nc\n");
        t.remove_empty_lines();
        assert_eq!(t.doc.text(), "a\nb\nc\n");
    }

    /// 4.6, сценарий «Верхний регистр»: буквы выделения заглавные,
    /// длина и остальной текст не меняются; прочие режимы регистра.
    #[test]
    fn line_ops_change_case() {
        let mut t = BufferTab::from_text("say hello, world! ok");
        t.set_caret_at(0, 4);
        t.select_to(0, 17); // "hello, world!"
        t.change_case(editor_core::CaseMode::Upper);
        assert_eq!(t.doc.text(), "say HELLO, WORLD! ok");
        assert_eq!(t.doc.len_chars(), 20, "длина не меняется");
        // Нижний.
        t.change_case(editor_core::CaseMode::Lower); // на том же выделении
        assert_eq!(t.doc.text(), "say hello, world! ok");
        // Без выделения — весь файл.
        let mut t = BufferTab::from_text("one Two\nthree FOUR\n");
        t.change_case(editor_core::CaseMode::Upper);
        assert_eq!(t.doc.text(), "ONE TWO\nTHREE FOUR\n");
        // Title / Sentence — весь файл.
        let mut t = BufferTab::from_text("mixed case text here\n");
        t.change_case(editor_core::CaseMode::Title);
        assert_eq!(t.doc.text(), "Mixed Case Text Here\n");
        let mut t = BufferTab::from_text("first one. second one! THIRD?\n");
        t.change_case(editor_core::CaseMode::Sentence);
        assert_eq!(t.doc.text(), "First one. Second one! Third?\n");
        // Undo работает для регистра.
        assert!(t.undo());
        assert_eq!(t.doc.text(), "first one. second one! THIRD?\n");
    }

    /// Ввод группируется: серия одиночных вводов — один undo.
    #[test]
    fn typing_grouped_into_single_undo() {
        let mut t = BufferTab::untitled();
        for ch in "abc".chars() {
            t.insert_text(&ch.to_string());
        }
        assert!(t.undo());
        assert_eq!(t.doc.text(), "");
        assert!(!t.undo());
    }

    /// Backspace / forward delete / enter.
    #[test]
    fn backspace_delete_enter() {
        let mut t = BufferTab::untitled();
        t.insert_text("ab");
        t.backspace(); // удаляет 'b'
        assert_eq!(t.doc.text(), "a");
        t.insert_text("\n");
        assert_eq!(t.doc.text(), "a\n");
        t.set_caret_at(0, 0);
        t.delete_forward(); // удаляет 'a'
        assert_eq!(t.doc.text(), "\n");

        // Границы: backspace в 0 и delete в конце — no-op. End ставит
        // каретку перед терминатором строки, поэтому для no-op delete
        // каретку в конец документа ставим явно.
        t.move_home(false);
        let before = t.doc.text();
        t.backspace();
        assert_eq!(t.doc.text(), before);
        t.set_caret_at(t.doc.len_lines() - 1, t.line_len(t.doc.len_lines() - 1));
        t.delete_forward();
        assert_eq!(t.doc.text(), before);
    }

    /// Навигация: стрелки/Home/End по многострочному документу,
    /// включая clamp столбца на короткой строке.
    #[test]
    fn cursor_movement() {
        let mut t = BufferTab::from_text("one\nthree\nxx\n");
        // "one"=3 chars, line0; "three"=5, line1; "xx"=2, line2.
        t.set_caret_at(1, 3); // line1, col3

        t.move_home(false);
        assert_eq!((t.cursor_line(), t.cursor_col()), (1, 0));
        t.move_end(false);
        assert_eq!((t.cursor_line(), t.cursor_col()), (1, 5));

        t.move_up(false); // col 5 -> clamp на "one" (len 3)
        assert_eq!((t.cursor_line(), t.cursor_col()), (0, 3));
        t.move_up(false); // clamp на строку 0
        assert_eq!(t.cursor_line(), 0);
        t.move_down(false);
        t.move_down(false); // на "xx"
        t.move_end(false);
        assert_eq!((t.cursor_line(), t.cursor_col()), (2, 2));
        t.move_left(false);
        assert_eq!(t.cursor_col(), 1);
        t.move_right(false);
        t.move_right(false); // clamp в конец документа
        assert_eq!(t.primary().head(), t.doc.len_chars());
    }

    /// 4.2: goal column (D9.2) — вертикальное движение помнит столбец,
    /// горизонтальное движение и клик его сбрасывают.
    #[test]
    fn goal_column_vertical_navigation() {
        let mut t = BufferTab::from_text("longline10\nhi\nmidline\n");
        // Строки: 0="longline10"(10), 1="hi"(2), 2="midline"(7).
        t.set_caret_at(0, 10); // конец строки 0, goal = None
        t.move_down(false); // clamp до 2, goal := 10
        assert_eq!((t.cursor_line(), t.cursor_col()), (1, 2));
        t.move_down(false); // goal 10 → clamp до 7 на "midline"
        assert_eq!((t.cursor_line(), t.cursor_col()), (2, 7));
        t.move_up(false); // goal 10 → clamp до 2 на "hi"
        assert_eq!((t.cursor_line(), t.cursor_col()), (1, 2));
        // Горизонтальное движение сбрасывает goal: было (1,2), после
        // move_left голова в col 1, вверх даст col 1.
        t.move_left(false);
        t.move_up(false);
        assert_eq!((t.cursor_line(), t.cursor_col()), (0, 1));
        // Клик тоже сбрасывает.
        t.set_caret_at(0, 10);
        t.move_down(false);
        t.set_caret_at(2, 0);
        t.move_up(false);
        assert_eq!((t.cursor_line(), t.cursor_col()), (1, 0));
    }

    /// 4.2: PgUp/PgDn — страница = переданное число строк, clamp по
    /// границам, столбец сохраняется. Документ: 5 строк + пустая
    /// хвостовая (терминатор в конце — отдельная строка 5).
    #[test]
    fn move_page_navigation() {
        let mut t = BufferTab::from_text("abcdef\nx\nyy\nzzz\nwwww\n");
        t.set_caret_at(0, 3);
        t.move_page(1, 3, false); // строка 0 + 3 → строка 3
        assert_eq!((t.cursor_line(), t.cursor_col()), (3, 3));
        t.move_page(10, 3, false); // clamp в последнюю (пустую хвостовую)
        assert_eq!((t.cursor_line(), t.cursor_col()), (5, 0));
        t.move_page(-1, 3, false); // goal 3 → clamp до 2 на "yy"
        assert_eq!((t.cursor_line(), t.cursor_col()), (2, 2));
        t.move_page(-10, 3, false); // clamp в первую
        assert_eq!((t.cursor_line(), t.cursor_col()), (0, 3));
    }

    /// 4.2: клик → каретка; shift-клик → расширение выделения от якоря.
    #[test]
    fn click_and_select_to() {
        // "alpha\nbeta\ngamma\n": line1 стартует с 6, line2 — с 11.
        let mut t = BufferTab::from_text("alpha\nbeta\ngamma\n");
        t.set_caret_at(1, 2); // 't' в "beta", позиция 8
        assert_eq!(t.primary().head(), 8);
        // Клик дальше конца строки — clamp в её длину.
        t.set_caret_at(1, 99);
        assert_eq!((t.cursor_line(), t.cursor_col()), (1, 4));
        // Строка вне документа — clamp в последнюю.
        t.set_caret_at(99, 1);
        assert_eq!((t.cursor_line(), t.cursor_col()), (3, 0));
        // Расширение: якорь 11 (line2 col0), голова 10 (line1 col4) —
        // reversed [10..11).
        t.set_caret_at(2, 0);
        t.select_to(1, 4);
        assert_eq!(*t.primary(), Selection::range(11, 10));
        assert!(t.primary().reversed);
        // Расширение дальше влево: якорь 11 сохраняется, голова 1 →
        // [1..11), голова слева.
        t.select_to(0, 1);
        assert_eq!(*t.primary(), Selection::range(11, 1));
        assert!(t.primary().reversed);
        assert_eq!(t.primary().head(), 1);
    }

    /// 4.2: shift-стрелки расширяют выделение главной каретки.
    #[test]
    fn shift_arrows_extend_selection() {
        let mut t = BufferTab::from_text("abcdef");
        t.set_caret_at(0, 1);
        t.move_right(true);
        t.move_right(true);
        assert_eq!(*t.primary(), Selection::range(1, 3));
        t.move_left(true); // голова назад: [1..2), голова всё ещё справа
        assert_eq!(*t.primary(), Selection::range(1, 2));
        assert!(!t.primary().reversed);
        t.move_left(true); // голова = якорю: схлопывается
        assert_eq!(*t.primary(), Selection::range(1, 1));
        assert!(!t.primary().reversed);
        t.move_left(true); // голова левее якоря — reversed [0..1)
        assert_eq!(*t.primary(), Selection::range(1, 0));
        assert!(t.primary().reversed);
        // Движение без shift сбрасывает выделение в каретку (в голове).
        t.move_right(false);
        assert_eq!(*t.primary(), Selection::caret(1));
    }

    impl BufferTab {
        /// Хелпер тестов: документ из текста (документ как есть).
        fn from_text(s: &str) -> Self {
            let mut t = BufferTab::untitled();
            t.doc = Document::from_text(s);
            t
        }
    }
}
