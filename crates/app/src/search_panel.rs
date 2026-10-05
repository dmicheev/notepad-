//! Панель поиска и замены (группа 6, D13): постоянная пока открыта
//! (Cmd+F, паттерн VS Code — НЕ модальное окно), отдельный FocusHandle
//! (ловушка D9.2 — клавиши панели не доходят до handle_key редактора).
//! Esc — закрыть, Enter — следующее совпадение/запуск поиска,
//! Shift+Enter — предыдущее.
//!
//! Области (scope): File — живой поиск по активной вкладке с подсветкой
//! ВСЕХ совпадений в рендере и счётчиком «текущее/N»; Tabs — по всем
//! открытым вкладкам (включая untitled, контент в памяти); Folder —
//! фоновый memmap+rayon-скан (движок editor-core::search), потоковые
//! батчи через канал, UI не блокируется. Результаты Tabs/Folder — секция
//! SEARCH левой панели (по образцу BOOKMARKS, D9.4).
//!
//! Замена: в файле — по одному (пересчёт после каждой правки — D13) и
//! Replace All одной составной транзакцией (undo одним шагом); по
//! файлам — с подтверждением `Window::prompt`, применение В ПАМЯТЬ
//! (затронутые файлы открываются вкладками dirty, диск не трогается),
//! повторный поиск видит содержимое вкладок, не диска.

use std::path::{Path, PathBuf};

use editor_core::search::SearchMatcher;
use editor_core::{ScanSummary, SearchOptions, Selection};
use gpui::{Context, KeyDownEvent, PathPromptOptions, Window};

use crate::workspace::Workspace;

/// Область поиска.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SearchScope {
    /// Активная вкладка (живой поиск + подсветка в рендере).
    File,
    /// Все открытые вкладки (untitled — контент в памяти).
    Tabs,
    /// Папка (фоновый скан; путь выбан диалогом или дефолтом — папка
    /// активной вкладки, D13).
    Folder(PathBuf),
}

/// Активное поле ввода панели (клик по полю выбирает).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchField {
    Query,
    Replace,
    Mask,
}

/// Владелец результата поиска.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SearchOwner {
    /// Открытая вкладка (tabs scope); untitled — индекс + подпись.
    Tab { index: usize, label: String },
    /// Файл папки (folder scope); клик — open_path с дедупликацией.
    Path { path: PathBuf, label: String },
}

/// Один результат поиска (строка секции SEARCH).
#[derive(Clone, Debug)]
pub(crate) struct SearchHit {
    pub owner: SearchOwner,
    /// 0-based строка.
    pub line: usize,
    /// 0-based char-колонка совпадения.
    pub col: usize,
    /// Длина совпадения (char).
    pub len: usize,
    /// Фрагмент строки и позиция совпадения в нём.
    pub fragment: String,
    pub frag_col: usize,
    pub frag_len: usize,
}

/// Состояние панели поиска.
pub struct SearchPanel {
    pub query: String,
    pub replacement: String,
    /// Маска файлов (folder scope): `*.rs`, `*.{ts,tsx}`; пусто — все.
    pub mask: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex_mode: bool,
    pub show_replace: bool,
    pub scope: SearchScope,
    pub field: SearchField,
    /// Совпадения активной вкладки (File scope): char-диапазоны.
    pub(crate) matches: Vec<(usize, usize)>,
    /// Индекс текущего совпадения (для счётчика/подсветки).
    pub(crate) current: usize,
    /// view_version вкладки на момент пересчёта (инвалидация, D13).
    version: u64,
    /// Ошибка компиляции regex (сообщение в панели и статус-баре).
    pub error: Option<String>,
    /// Результаты (Tabs/Folder).
    pub results: Vec<SearchHit>,
    /// Фоновый скан идёт.
    pub scanning: bool,
    /// Итог завершённого скана.
    pub summary: Option<ScanSummary>,
    /// Результаты усечены капом (MAX_HITS_*).
    pub truncated: bool,
}

impl SearchPanel {
    fn new(scope: SearchScope) -> Self {
        SearchPanel {
            query: String::new(),
            replacement: String::new(),
            mask: String::new(),
            case_sensitive: false,
            whole_word: false,
            regex_mode: false,
            show_replace: false,
            scope,
            field: SearchField::Query,
            matches: Vec::new(),
            current: 0,
            version: 0,
            error: None,
            results: Vec::new(),
            scanning: false,
            summary: None,
            truncated: false,
        }
    }

    /// Счётчик панели: «текущее/N» или «No results».
    pub fn counter(&self) -> String {
        if self.matches.is_empty() {
            "No results".to_string()
        } else {
            format!("{}/{}", (self.current + 1).min(self.matches.len()), self.matches.len())
        }
    }

    /// Совпадения активной вкладки (для подсветки рендера).
    pub fn file_matches(&self) -> &[(usize, usize)] {
        &self.matches
    }

    /// Диапазон текущего совпадения (подсветка «текущего»).
    pub fn current_match(&self) -> Option<(usize, usize)> {
        self.matches.get(self.current).copied()
    }
}

impl Workspace {
    // ---- Открытие/закрытие (действия 6.1/6.3/6.4) ----

    /// Cmd+F: панель поиска по файлу; непустое выделение в одной
    /// строке предзаполняет запрос (рекомендация задачника).
    pub(crate) fn on_find_in_file(
        &mut self,
        _: &crate::actions::FindInFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.model.active().is_none() {
            return;
        }
        let mut panel = SearchPanel::new(SearchScope::File);
        if let Some(tab) = self.model.active() {
            let sel = tab.primary();
            if !sel.is_caret()
                && tab.doc.char_to_line(sel.start) == tab.doc.char_to_line(sel.end)
            {
                panel.query = tab.doc.slice_str(sel.start, sel.end);
            }
        }
        self.open_search(panel, window, cx);
    }

    /// Cmd+Shift+F: панель поиска по открытым вкладкам.
    pub(crate) fn on_find_in_tabs(
        &mut self,
        _: &crate::actions::FindInTabs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.model.active().is_none() {
            return;
        }
        self.open_search(SearchPanel::new(SearchScope::Tabs), window, cx);
    }

    /// Меню Search → Find in Folder…: диалог выбора папки
    /// (`PathPromptOptions { directories: true }`).
    pub(crate) fn on_find_in_folder(
        &mut self,
        _: &crate::actions::FindInFolder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose folder to search".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(folder) = paths.into_iter().next() {
                    this.update_in(cx, |ws, window, cx| {
                        ws.open_search(SearchPanel::new(SearchScope::Folder(folder)), window, cx);
                    })
                    .ok();
                }
        })
        .detach();
    }

    fn open_search(
        &mut self,
        panel: SearchPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_file = panel.scope == SearchScope::File;
        self.search = Some(panel);
        window.focus(&self.search_focus);
        if is_file {
            self.search_recompute_file();
        }
        cx.notify();
    }

    /// Программный запуск folder-поиска (диалог ОС недоступен в
    /// gpui::test): устанавливает scope и открывает панель.
    #[allow(dead_code)] // API для GUI-тестов (паттерн D10.4: честное API модели)
    pub(crate) fn open_folder_search(
        &mut self,
        folder: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_search(SearchPanel::new(SearchScope::Folder(folder)), window, cx);
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search = None;
        window.focus(&self.focus);
        cx.notify();
    }

    // ---- Клавиатура панели ----

    /// Клавиши панели: Esc — закрыть; Enter — следующее (File) /
    /// запуск поиска (Tabs/Folder); Shift+Enter — предыдущее; ввод —
    /// в активное поле (File scope: живой пересчёт). Висит на панели
    /// (её фокус — редактор клавиш не получает, ловушка D9.2).
    pub(crate) fn handle_search_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        // Клавиши панели НЕ всплывают в контейнер редактора (панель —
        // его ребёнок; on_key_down редактора ниже по дереву получил бы
        // ввод и правилал бы документ — ловушка поймана GUI-тестом).
        cx.stop_propagation();
        if ks.modifiers.platform || ks.modifiers.control || ks.modifiers.alt {
            return;
        }
        let Some(sp) = &mut self.search else { return };
        let scope_file = sp.scope == SearchScope::File;
        match ks.key.as_str() {
            "escape" => {
                self.close_search(window, cx);
                return;
            }
            "enter" => {
                if ks.modifiers.shift && self.search.as_ref().is_some_and(|sp| {
                    sp.scope == SearchScope::File
                }) {
                    self.search_jump(false);
                } else {
                    self.search_enter(window, cx);
                }
            }
            "backspace" => {
                let field = active_field_mut(sp);
                field.pop();
                if scope_file {
                    self.search_recompute_file();
                }
            }
            "tab" => {
                // Циклическое переключение поля ввода: Query → Replace
                // (если раскрыто) → Mask (folder) → Query.
                let is_folder = matches!(sp.scope, SearchScope::Folder(_));
                sp.field = match (sp.field, is_folder, sp.show_replace) {
                    (SearchField::Query, false, true) => SearchField::Replace,
                    (SearchField::Query, true, _) => SearchField::Mask,
                    (SearchField::Query, false, false) => SearchField::Query,
                    (SearchField::Replace, true, _) => SearchField::Mask,
                    (SearchField::Replace, false, _) => SearchField::Query,
                    (SearchField::Mask, _, _) => SearchField::Query,
                };
            }
            _ => {
                if let Some(ch) = ks
                    .key_char
                    .as_deref()
                    .filter(|c| !c.chars().any(|c| c.is_control()))
                {
                    let field = active_field_mut(sp);
                    if field.len() < 512 {
                        field.push_str(ch);
                    }
                    if scope_file {
                        self.search_recompute_file();
                    }
                }
            }
        }
        cx.notify();
    }

    // ---- File scope: пересчёт/инвалидация/навигация ----

    /// Компилятор из состояния панели; None — пустой запрос (no-op),
    /// Err — сообщение об ошибке regex (не паника).
    pub(crate) fn search_matcher(sp: &SearchPanel) -> Option<Result<SearchMatcher, String>> {
        if sp.query.trim().is_empty() {
            return None;
        }
        Some(SearchMatcher::new(
            &sp.query,
            SearchOptions {
                case_sensitive: sp.case_sensitive,
                whole_word: sp.whole_word,
                regex: sp.regex_mode,
            },
        ))
    }

    /// Полный пересчёт совпадений активной вкладки (File scope):
    /// смена запроса/опций/вкладки, undo/redo. Ошибка regex — в панель
    /// и статус-бар.
    pub(crate) fn search_recompute_file(&mut self) {
        let Some(sp) = self.search.as_mut() else { return };
        if sp.scope != SearchScope::File {
            return;
        }
        sp.error = None;
        sp.matches.clear();
        sp.current = 0;
        let Some(tab) = self.model.active() else { return };
        match Self::search_matcher(sp) {
            None => {}
            Some(Ok(m)) => {
                sp.matches = m.find_in_doc(&tab.doc);
            }
            Some(Err(e)) => {
                sp.error = Some(e);
            }
        }
        sp.version = tab.view_version();
        search_update_current(sp, tab);
        if let Some(err) = sp.error.clone() {
            self.status_message = Some(format!("Invalid search pattern: {err}"));
        }
    }

    /// Синхронизация совпадений с правками активной вкладки (D13):
    /// версия изменилась — частичная инвалидация от строки правки
    /// (`take_last_edit_line` — apply_tx её ставит), undo/redo без
    /// строки — полный пересчёт. Вызывается из рендера панели.
    pub(crate) fn sync_search_matches(&mut self) {
        let Some(sp) = self.search.as_mut() else { return };
        if sp.scope != SearchScope::File {
            return;
        }
        let Some(tab) = self.model.active_mut() else {
            sp.matches.clear();
            return;
        };
        if sp.version == tab.view_version() {
            return;
        }
        let edited_line = tab.take_last_edit_line();
        sp.error = None;
        match edited_line {
            Some(line) => {
                // Совпадения, полностью лежащие до строки правки,
                // валидны (позиции не сдвинуты); хвост пересчитывается
                // от начала строки правки (overlap назад — внутри
                // движка, find_in_doc_from).
                let from_char = tab.doc.line_to_char(line);
                let matcher = Self::search_matcher(sp);
                let kept: Vec<(usize, usize)> = sp
                    .matches
                    .iter()
                    .copied()
                    .filter(|&(_, e)| e <= from_char)
                    .collect();
                let tail = match matcher {
                    Some(Ok(m)) => m.find_in_doc_from(&tab.doc, from_char),
                    _ => Vec::new(),
                };
                let mut merged = kept;
                merged.extend(tail);
                sp.matches = merged;
            }
            None => {
                sp.matches.clear();
                if let Some(Ok(m)) = Self::search_matcher(sp) {
                    sp.matches = m.find_in_doc(&tab.doc);
                }
            }
        }
        sp.version = tab.view_version();
        search_update_current(sp, tab);
    }

    /// Навигация: следующее/предыдущее совпадение от ГЛАВНОЙ каретки,
    /// циклически; каретка — выделение совпадения + scroll_to_line
    /// (скрытая строка поднимается к заголовку фолда внутри скролла).
    /// Enter/лупа: File scope — прыжок к следующему совпадению;
    /// Tabs/Folder — запуск поиска (идущий скан не перезапускаем).
    pub(crate) fn search_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let is_file = self
            .search
            .as_ref()
            .is_some_and(|sp| sp.scope == SearchScope::File);
        if is_file {
            self.search_jump(true);
            cx.notify();
            return;
        }
        if self.search.as_ref().is_some_and(|sp| sp.scanning) {
            return;
        }
        self.run_tabs_or_folder_search(window, cx);
    }

    pub(crate) fn search_jump(&mut self, forward: bool) {
        self.sync_search_matches();
        let Some(sp) = self.search.as_mut() else { return };
        if sp.matches.is_empty() {
            return;
        }
        let Some(tab) = self.model.active() else { return };
        let primary = *tab.primary();
        // Точка отсчёта навигации: каретка — от головы; выделенное
        // совпадение (после прыжка) — от его конца/начала: Enter от
        // выделенного k даёт k+1, Shift+Enter — k−1 (VS Code семантика).
        let from = if primary.is_caret() { primary.head() } else { primary.end };
        let idx = if forward {
            // Первое совпадение, начинающееся не раньше точки отсчёта;
            // исчерпание — циклически первое.
            sp.matches
                .iter()
                .position(|&(s, _)| s >= from)
                .unwrap_or(0)
        } else {
            // Последнее совпадение, ЗАКАНЧИВАЮЩЕЕСЯ до точки отсчёта
            // (строго — выделенное само себя не находит); исчерпание —
            // циклически последнее.
            sp.matches
                .iter()
                .rposition(|&(_, e)| e < from)
                .unwrap_or(sp.matches.len() - 1)
        };
        sp.current = idx;
        let (s, e) = sp.matches[idx];
        let line = tab.doc.char_to_line(s);
        if let Some(tab) = self.model.active_mut() {
            tab.set_cursors(vec![Selection::range(s, e)]);
        }
        // Немедленный скролл (замечание юзабилити): строка совпадения
        // обязана быть в вьюпорте СРАЗУ после прыжка — deferred-скролл
        // опаздывал на кадр (ловушка D12.5).
        self.scroll_to_line_now(line);
    }

    // ---- Замена в файле (6.2) ----

    /// Replace (текущее) / Replace All по активной вкладке. Правки —
    /// составной транзакцией через `apply_search_edits` (undo одним
    /// шагом). Replace по одному: пересчёт замен после каждой правки
    /// (решение D13). `true` — была хотя бы одна замена.
    pub(crate) fn search_replace(&mut self, all: bool, cx: &mut Context<Self>) {
        self.sync_search_matches();
        let Some(sp) = self.search.as_ref() else { return };
        if sp.scope != SearchScope::File || sp.query.trim().is_empty() {
            return;
        }
        let Some(Ok(matcher)) = Self::search_matcher(sp) else { return };
        let replacement = sp.replacement.clone();
        let current = sp.current;
        let Some(tab) = self.model.active_mut() else { return };
        let Ok(mut edits) = matcher.replacements_in_doc(&tab.doc, &replacement) else {
            return;
        };
        if !all {
            if current >= edits.len() {
                return;
            }
            edits.drain(..current); // текущее + хвост
            edits.truncate(1); // только текущее
        }
        // Каретка — за вставкой текущей замены (VS Code поведение).
        let caret_after = edits
            .first()
            .map(|(s, _, t)| s + t.chars().count());
        let n = tab.apply_search_edits(edits);
        if let Some(pos) = caret_after.filter(|_| !all) {
            let len = tab.doc.len_chars();
            tab.set_cursors(vec![Selection::caret(pos.min(len))]);
        }
        if n > 0 {
            self.status_message =
                Some(format!("Replaced {} occurrence(s)", n));
        }
        self.search_recompute_file();
        cx.notify();
    }

    // ---- Переход по результату (6.3/6.4) ----

    /// Клик по результату SEARCH: вкладка (или open_path файла) +
    /// выделение совпадения + scroll_to_line (свернутое тело — к
    /// заголовку фолда, visible_line_or_header внутри скролла).
    pub(crate) fn goto_search_hit(&mut self, hit_ix: usize, cx: &mut Context<Self>) {
        let Some(sp) = self.search.as_ref() else { return };
        let Some(hit) = sp.results.get(hit_ix).cloned() else { return };
        let ix = match &hit.owner {
            SearchOwner::Tab { index, .. } => {
                if *index >= self.model.len() {
                    return; // вкладка закрыта с момента поиска
                }
                self.model.activate(*index);
                *index
            }
            SearchOwner::Path { path, .. } => match self.model.open_path(path) {
                Ok(ix) => ix,
                Err(e) => {
                    self.status_message =
                        Some(format!("Could not open {}: {e}", path.display()));
                    cx.notify();
                    return;
                }
            },
        };
        let line = hit.line;
        if let Some(tab) = self.model.tab_mut(ix) {
            let last = tab.doc.len_lines().saturating_sub(1);
            let line = line.min(last);
            let ls = tab.doc.line_to_char(line);
            let line_len = tab.doc.line_content(line).chars().count();
            let start = ls + hit.col.min(line_len);
            let end = ls + (hit.col + hit.len).min(line_len);
            tab.set_cursors(vec![Selection::range(start, end.max(start))]);
        }
        // Немедленный скролл к строке совпадения (как в прыжках ‹/›).
        self.scroll_to_line_now(line);
        cx.notify();
    }

    // ---- Тумблеры/кнопки панели (клики) ----

    pub(crate) fn on_search_toggle(
        &mut self,
        what: &str,
        _e: &gpui::MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let recompute = {
            let Some(sp) = self.search.as_mut() else { return };
            match what {
                "case" => sp.case_sensitive = !sp.case_sensitive,
                "word" => sp.whole_word = !sp.whole_word,
                "regex" => sp.regex_mode = !sp.regex_mode,
                "replace" => sp.show_replace = !sp.show_replace,
                _ => return,
            }
            sp.scope == SearchScope::File
        };
        if recompute {
            self.search_recompute_file();
        }
        cx.notify();
    }

    /// Клик по кнопке навигации ‹ ›.
    pub(crate) fn on_search_nav(
        &mut self,
        forward: bool,
        _e: &gpui::MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_jump(forward);
        cx.notify();
    }

    /// Клик по чипу scope: File/Tabs — переключение; Folder — дефолт
    /// папка активной вкладки (есть путь) либо диалог (D13).
    pub(crate) fn on_search_scope(
        &mut self,
        scope: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match scope {
            "file" => {
                if let Some(sp) = self.search.as_mut() {
                    sp.scope = SearchScope::File;
                }
                self.search_recompute_file();
                cx.notify();
            }
            "tabs" => {
                if let Some(sp) = self.search.as_mut() {
                    sp.scope = SearchScope::Tabs;
                    sp.results.clear();
                    sp.summary = None;
                }
                cx.notify();
            }
            "folder" => {
                // Дефолт — папка активной вкладки; иначе диалог.
                let default = self
                    .model
                    .active()
                    .and_then(|t| t.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())));
                if let Some(folder) = default {
                    if let Some(sp) = self.search.as_mut() {
                        sp.scope = SearchScope::Folder(folder);
                        sp.results.clear();
                        sp.summary = None;
                    }
                    cx.notify();
                } else {
                    self.on_find_in_folder(&crate::actions::FindInFolder, window, cx);
                }
            }
            _ => {}
        }
    }

    /// Клик по кнопке «…» рядом с маской — смена папки диалогом.
    pub(crate) fn on_search_choose_folder(
        &mut self,
        _e: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_find_in_folder(&crate::actions::FindInFolder, window, cx);
    }

    /// Клик по полю ввода — сделать его активным.
    pub(crate) fn on_search_field(
        &mut self,
        field: SearchField,
        _e: &gpui::MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(sp) = self.search.as_mut() {
            sp.field = field;
        }
        cx.notify();
    }

    pub(crate) fn on_search_close(
        &mut self,
        _e: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_search(window, cx);
    }

    pub(crate) fn on_search_replace_button(
        &mut self,
        all: bool,
        _e: &gpui::MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_replace(all, cx);
    }

    /// Совпадения строки для подсветки рендера (File scope):
    /// col-диапазоны пересечений со строкой + признак текущего.
    /// Бинарный поиск по отсортированным совпадениям — O(log n + k),
    /// не O(все совпадения) на каждую видимую строку кадра.
    pub(crate) fn search_line_hits(&self, real_line: usize) -> (Vec<(usize, usize)>, Vec<bool>) {
        let Some(sp) = self.search.as_ref() else { return (Vec::new(), Vec::new()) };
        if sp.scope != SearchScope::File {
            return (Vec::new(), Vec::new());
        }
        let Some(tab) = self.model.active() else { return (Vec::new(), Vec::new()) };
        let last = tab.doc.len_lines().saturating_sub(1);
        let real_line = real_line.min(last);
        let ls = tab.doc.line_to_char(real_line);
        let le = ls + tab.doc.line_content(real_line).chars().count();
        let matches = sp.file_matches();
        let cur = sp.current_match();
        let mut ranges = Vec::new();
        let mut current = Vec::new();
        let mut i = matches.partition_point(|&(s, _)| s < ls);
        while let Some(&(s, e)) = matches.get(i) {
            if s >= le {
                break;
            }
            let a = s.max(ls);
            let b = e.min(le);
            if a < b {
                ranges.push((a - ls, b - ls));
                current.push(cur == Some((s, e)));
            }
            i += 1;
        }
        (ranges, current)
    }
}

/// События фонового скана → UI-задаче (std-канал + поллинг таймером).
pub(crate) enum ScanEvent {
    Batch(Vec<editor_core::search::FileScan>),
    Done(ScanSummary),
}

/// Активное текстовое поле панели.
fn active_field_mut(sp: &mut SearchPanel) -> &mut String {
    match sp.field {
        SearchField::Query => &mut sp.query,
        SearchField::Replace => &mut sp.replacement,
        SearchField::Mask => &mut sp.mask,
    }
}

/// Текущее совпадение — ближайшее к главной каретке (счётчик/подсветка).
fn search_update_current(sp: &mut SearchPanel, tab: &crate::buffer_tab::BufferTab) {
    if sp.matches.is_empty() {
        sp.current = 0;
        return;
    }
    let head = tab.primary().head();
    sp.current = sp
        .matches
        .iter()
        .position(|&(_, e)| e > head)
        .unwrap_or(sp.matches.len() - 1);
}

/// Фрагмент строки вокруг совпадения для секции SEARCH (контент строки
/// передан отдельно: фоновый поиск по вкладкам работает с rope, не с
/// `Document`).
pub(crate) fn fragment_from_content(content: &str, col: usize, len: usize) -> (String, usize, usize) {
    let chars = content.chars().count();
    let frag_start = col.saturating_sub(30);
    let frag_end = (col + len + editor_core::search::FRAGMENT_MAX_CHARS).min(chars);
    let fragment: String = content
        .chars()
        .skip(frag_start)
        .take(frag_end - frag_start)
        .collect();
    let frag_len = len.min(frag_end - col);
    (fragment, col - frag_start, frag_len)
}

// ---- Рендер ----

/// Ширина панели поиска (px).
pub(crate) const SEARCH_PANEL_W: f32 = 560.0;

/// Кап результатов поиска по вкладкам: на вкладку (баг ручной
/// проверки: сотни тысяч `SearchHit` убивали UI) и суммарно.
pub(crate) const MAX_HITS_PER_TAB: usize = 500;
pub(crate) const MAX_HITS_TOTAL: usize = 2000;
/// Кап отображаемых строк секции SEARCH (панель не virtualized —
/// стройка тысяч div на каждый кадр фризила UI).
pub(crate) const SEARCH_SECTION_MAX_ROWS: usize = 300;

/// Стабильный числовой ключ пути (счёт «файлов с совпадениями»).
pub(crate) fn path_hash(p: &Path) -> usize {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.hash(&mut h);
    h.finish() as usize
}
