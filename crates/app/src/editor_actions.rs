//! Обработчики действий (меню/горячие клавиши) и клавиатурный ввод
//! редактора (группы 3–4 + 8.1): файлы (open/save/close), split view,
//! undo/redo, закладки, свёртывание, операции со строками, Cmd+D.
//! Регистрация on_action — на корне (workspace.rs::render); типы
//! действий — actions.rs; биндинги — main.rs.
//!
//! Все действия работают с АКТИВНОЙ панелью (`model_mut()`); клик по
//! панели/вкладке активирует её (pane.rs).

use std::path::PathBuf;

use gpui::{Context, KeyDownEvent, PathPromptOptions, PromptLevel, Window};

use crate::actions::*;
use crate::workspace::Workspace;

impl Workspace {
    // ---- Действия (минимальный набор группы 3; полный справочник — 8.5) ----

    /// Cmd+O: диалог ОС (gpui 0.2.2 `prompt_for_paths`; rfd не нужен —
    /// D8.3), открытие каждого файла обычной вкладкой в активную панель.
    pub(crate) fn on_open(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open files".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update_in(cx, |ws, window, cx| ws.open_paths(&paths, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Открыть пути (дедупликация — в модели); ошибки — в статус-бар.
    /// Файлы сверх порога 20 МБ — вопрос пользователю о подсветке
    /// (D12.4): согласие включает её для любого размера.
    pub fn open_paths(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let pi = self.active_pane;
        let mut first_error: Option<String> = None;
        let mut opened: Vec<usize> = Vec::new();
        for path in paths {
            match self.panes[pi].model.open_path(path) {
                Ok(i) => opened.push(i),
                Err(e) => {
                    let msg = crate::i18n::tf(
                        cx,
                        "status.open_failed",
                        &[("file", &path.display().to_string()), ("err", &e.to_string())],
                    );
                    eprintln!("{msg}");
                    first_error.get_or_insert(msg);
                }
            }
        }
        if let Some(msg) = first_error {
            self.status_message = Some(msg);
        }
        // Порог подсветки: спросить про новые крупные файлы.
        let over: Vec<usize> = opened
            .iter()
            .copied()
            .filter(|&i| {
                self.panes[pi]
                    .model
                    .tab_mut(i)
                    .is_some_and(|t| t.language.over_threshold && !t.language.threshold_allowed)
            })
            .collect();
        if !over.is_empty() && !crate::bench::enabled() {
            self.pending_threshold_tabs
                .extend(over.iter().map(|&i| (pi, i)));
            self.ask_pending_threshold(window, cx);
        }
        // Фоновая раскраска новых вкладок чекпоинтами (сразу, не
        // дожидаясь прыжков пользователя; workspace.rs).
        for i in &opened {
            self.spawn_background_highlight(pi, *i, cx);
        }
        cx.notify();
    }

    /// Cmd+S: сохранить активную вкладку; untitled → save-as диалог.
    pub(crate) fn on_save(&mut self, _: &SaveFile, _window: &mut Window, cx: &mut Context<Self>) {
        let has_path = self
            .model()
            .active()
            .is_some_and(|t| t.path.is_some());
        if has_path {
            self.save_active(cx);
        } else if self.model().active().is_some() {
            self.save_as(cx);
        }
    }

    /// Cmd+Shift+S: сохранить как (в т.ч. первый Cmd+S для untitled).
    pub(crate) fn on_save_as(&mut self, _: &SaveFileAs, _window: &mut Window, cx: &mut Context<Self>) {
        if self.model().active().is_some() {
            self.save_as(cx);
        }
    }

    /// Сохранение по существующему пути; ошибка — в статус-бар.
    fn save_active(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.model_mut().active_mut()
            && let Err(e) = tab.save() {
                self.status_message =
                    Some(crate::i18n::tf(cx, "status.save_failed", &[("err", &e.to_string())]));
            }
        cx.notify();
    }

    /// Диалог сохранения (gpui `prompt_for_new_path`, D8.3).
    fn save_as(&mut self, cx: &mut Context<Self>) {
        let dir = self
            .model()
            .active()
            .and_then(|t| t.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())))
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let rx = cx.prompt_for_new_path(&dir, Some("Untitled.txt"));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                this.update(cx, |ws, cx| {
                    if let Some(tab) = ws.model_mut().active_mut()
                        && let Err(e) = tab.save_to(&path) {
                            ws.status_message = Some(crate::i18n::tf(
                                cx,
                                "status.save_failed",
                                &[("err", &e.to_string())],
                            ));
                        }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Cmd+W: закрыть вкладку; dirty — подтверждение через модальный
    /// gpui `Window::prompt` (Save / Don't Save / Cancel; D8.2).
    pub(crate) fn on_close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.model().active_index() {
            self.close_tab_in_pane(self.active_pane, index, window, cx);
        }
    }

    /// Закрыть вкладку панели `pi` (8.1): dirty — диалог
    /// Save / Don't Save / Cancel (D8.2); панель остаётся.
    pub(crate) fn close_tab_in_pane(
        &mut self,
        pi: usize,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dirty = self
            .panes
            .get(pi)
            .and_then(|p| p.model.tabs().get(index))
            .is_some_and(|t| t.dirty);
        if !dirty {
            if let Some(pane) = self.panes.get_mut(pi) {
                pane.model.close(index);
            }
            cx.notify();
            return;
        }
        let title = self.panes[pi]
            .model
            .title(index)
            .unwrap_or_else(|| "Untitled".into());
        let rx = window.prompt(
            PromptLevel::Warning,
            &crate::i18n::tf(cx, "dialog.dirty_title", &[("title", &title)]),
            Some(&crate::i18n::t(cx, "dialog.dirty_message")),
            &[
                crate::i18n::t(cx, "dialog.save").as_str(),
                crate::i18n::t(cx, "dialog.dont_save").as_str(),
                crate::i18n::t(cx, "dialog.cancel").as_str(),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = rx.await else { return };
            this.update_in(cx, |ws, _window, cx| match choice {
                0 => ws.save_and_close(pi, index, cx),
                1 => {
                    if let Some(pane) = ws.panes.get_mut(pi) {
                        pane.model.close(index);
                    }
                    cx.notify();
                }
                _ => {}
            })
            .ok();
        })
        .detach();
    }

    /// Сохранить и закрыть; untitled — сначала save-as (отмена диалога
    /// оставляет вкладку открытой).
    fn save_and_close(&mut self, pi: usize, index: usize, cx: &mut Context<Self>) {
        let has_path = self
            .panes
            .get(pi)
            .and_then(|p| p.model.tabs().get(index))
            .is_some_and(|t| t.path.is_some());
        if !has_path {
            self.save_as_then_close(pi, index, cx);
            return;
        }
        match self
            .panes
            .get_mut(pi)
            .and_then(|p| p.model.tab_mut(index))
            .map(|t| t.save())
        {
            Some(Ok(())) => {
                if let Some(pane) = self.panes.get_mut(pi) {
                    pane.model.close(index);
                }
                cx.notify();
            }
            Some(Err(e)) => self.status_message = Some(format!("Save failed: {e}")),
            None => {}
        }
    }

    fn save_as_then_close(&mut self, pi: usize, index: usize, cx: &mut Context<Self>) {
        let dir = self
            .panes
            .get(pi)
            .and_then(|p| p.model.tabs().get(index))
            .and_then(|t| t.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())))
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let rx = cx.prompt_for_new_path(&dir, Some("Untitled.txt"));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                this.update(cx, |ws, cx| {
                    let saved = ws
                        .panes
                        .get_mut(pi)
                        .and_then(|p| p.model.tab_mut(index))
                        .is_some_and(|t| t.save_to(&path).is_ok());
                    if saved
                        && let Some(pane) = ws.panes.get_mut(pi)
                    {
                        pane.model.close(index);
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    // ---- Темы (8.2/D15.2) ----

    /// Меню View → Theme: системная / тёмная / светлая. Выбор
    /// применяется немедленно (перерисовка) и сохраняется в сессии.
    pub(crate) fn on_theme_system(
        &mut self,
        _: &ThemeSystem,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_theme_kind(crate::theme::ThemeKind::System, cx);
    }

    pub(crate) fn on_theme_dark(
        &mut self,
        _: &ThemeDark,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_theme_kind(crate::theme::ThemeKind::Dark, cx);
    }

    pub(crate) fn on_theme_light(
        &mut self,
        _: &ThemeLight,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_theme_kind(crate::theme::ThemeKind::Light, cx);
    }

    fn set_theme_kind(&mut self, kind: crate::theme::ThemeKind, cx: &mut Context<Self>) {
        self.theme_kind = kind;
        // Немедленное применение: render разрулит по appearance, но
        // выставляем и здесь (панели читают current() в кадре).
        cx.notify();
    }

    // ---- Вкладки (8.5/D15.5): Cmd+T / Cmd+Shift+[ ] в активной панели ----

    /// Cmd+T: новая untitled-вкладка в активной панели.
    pub(crate) fn on_new_tab(&mut self, _: &NewTab, _window: &mut Window, cx: &mut Context<Self>) {
        self.model_mut().new_untitled();
        cx.notify();
    }

    /// Cmd+Shift+]: следующая вкладка активной панели (по кругу).
    pub(crate) fn on_next_tab(&mut self, _: &NextTab, _window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(1, cx);
    }

    /// Cmd+Shift+[: предыдущая вкладка (по кругу).
    pub(crate) fn on_prev_tab(&mut self, _: &PrevTab, _window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(-1, cx);
    }

    fn cycle_tab(&mut self, dir: isize, cx: &mut Context<Self>) {
        let pi = self.active_pane;
        let Some(model) = self.panes.get_mut(pi).map(|p| &mut p.model) else { return };
        let len = model.len();
        if len == 0 {
            return;
        }
        let cur = model.active_index().unwrap_or(0) as isize;
        let next = (cur + dir).rem_euclid(len as isize) as usize;
        model.activate(next);
        cx.notify();
    }

    // ---- Справочник сочетаний (8.5) ----

    /// Cmd+/ или Help-меню: открыть/закрыть оверлей-справочник.
    pub(crate) fn on_show_shortcuts(
        &mut self,
        _: &ShowShortcuts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shortcuts_open = !self.shortcuts_open;
        if self.shortcuts_open {
            let f = self.shortcuts_focus.clone();
            window.focus(&f);
        } else {
            let f = self.active_focus();
            window.focus(&f);
        }
        cx.notify();
    }

    /// Cmd+Shift+L: следующий язык интерфейса по кругу (8.5).
    pub(crate) fn on_cycle_language(
        &mut self,
        _: &CycleLanguage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let langs = crate::i18n::available(cx);
        let cur = self.ui_language.clone();
        let next = langs
            .iter()
            .position(|l| *l == cur)
            .map(|i| langs[(i + 1) % langs.len()].clone())
            .unwrap_or_else(|| langs.first().cloned().unwrap_or_else(|| "en".to_string()));
        crate::i18n::set_language(cx, &next);
        self.ui_language = next;
        cx.notify();
    }

    // ---- Split view (8.1/D15.1) ----

    /// Cmd+\: разделить редактор — новая панель (до MAX_PANES),
    /// активируется; файлы открываются в фокусированную панель.
    pub(crate) fn on_split_editor(
        &mut self,
        _: &SplitEditor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_pane(window, cx);
    }

    /// Cmd+Shift+\: закрыть активную панель (dirty-вкладки — диалоги;
    /// pane.rs), последняя панель не закрывается.
    pub(crate) fn on_close_pane(&mut self, _: &ClosePane, window: &mut Window, cx: &mut Context<Self>) {
        self.close_active_pane(window, cx);
    }

    /// Меню View: синхронная прокрутка панелей (8.1).
    pub(crate) fn on_toggle_sync_scroll(
        &mut self,
        _: &ToggleSyncScroll,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_sync_scroll(cx);
    }

    pub(crate) fn on_undo(&mut self, _: &Undo, _window: &mut Window, cx: &mut Context<Self>) {
        self.with_active_tab(|tab| { tab.undo(); }, cx);
    }

    pub(crate) fn on_redo(&mut self, _: &Redo, _window: &mut Window, cx: &mut Context<Self>) {
        self.with_active_tab(|tab| { tab.redo(); }, cx);
    }

    // ---- Клавиатурный ввод редактора (plain-клавиши; Cmd/Ctrl —
    // действия выше; список клавиш — D9.2/D9.3) ----

    /// Ввод в ФОКУСИРОВАННУЮ панель (8.1): listener редактора панели
    /// вызывает с её индексом; панель становится активной (паттерн
    /// VS Code — ввод переключает активную группу).
    pub(crate) fn handle_key_pane(
        &mut self,
        pi: usize,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        // Cmd/Ctrl-комбинации — действия (actions!); Option-символы
        // macOS не вводим. Shift не блокируем: key_char даёт верный
        // верхний регистр/символ.
        if ks.modifiers.platform || ks.modifiers.control || ks.modifiers.alt {
            return;
        }
        if pi != self.active_pane {
            self.activate_pane(pi, cx);
        }
        let extend = ks.modifiers.shift;
        let page = self.page_lines(window);
        let Some(tab) = self.panes.get_mut(pi).and_then(|p| p.model.active_mut()) else { return };
        match ks.key.as_str() {
            "backspace" => tab.backspace(),
            "delete" => tab.delete_forward(),
            "enter" => tab.insert_text("\n"),
            "tab" => tab.insert_text("\t"),
            "left" => tab.move_left(extend),
            "right" => tab.move_right(extend),
            "up" => tab.move_up(extend),
            "down" => tab.move_down(extend),
            "home" => tab.move_home(extend),
            "end" => tab.move_end(extend),
            // Страница = высота вьюпорта в строках (D9.2).
            "pageup" => tab.move_page(-1, page, extend),
            "pagedown" => tab.move_page(1, page, extend),
            _ => {
                let Some(ch) = ks
                    .key_char
                    .as_deref()
                    .filter(|c| !c.chars().any(|c| c.is_control()))
                else {
                    return;
                };
                tab.insert_text(ch);
            }
        }
        cx.notify();
    }

    // ---- Закладки (4.4) ----

    /// Cmd+F2: toggle закладки на строке главной каретки.
    pub(crate) fn on_toggle_bookmark(&mut self, _: &ToggleBookmark, _window: &mut Window, cx: &mut Context<Self>) {
        self.with_active_tab(|tab| tab.toggle_bookmark_on_primary(), cx);
    }

    /// F2: следующая закладка по кругу (+ скролл к ней).
    pub(crate) fn on_next_bookmark(&mut self, _: &NextBookmark, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.model_mut().active_mut()
            && tab.goto_next_bookmark() {
                let line = tab.cursor_line();
                self.scroll_to_line(line);
            }
        cx.notify();
    }

    /// Shift+F2: предыдущая закладка по кругу (+ скролл).
    pub(crate) fn on_prev_bookmark(&mut self, _: &PrevBookmark, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.model_mut().active_mut()
            && tab.goto_prev_bookmark() {
                let line = tab.cursor_line();
                self.scroll_to_line(line);
            }
        cx.notify();
    }

    /// Клик по маркеру закладки в гуттере строки.
    pub(crate) fn toggle_bookmark_at(&mut self, line: usize, cx: &mut Context<Self>) {
        self.with_active_tab(|tab| tab.toggle_bookmark_at(line), cx);
    }

    /// Клик по записи в панели закладок: каретка на строку + скролл.
    pub(crate) fn goto_bookmark(&mut self, line: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.model_mut().active_mut() {
            tab.set_caret_at(line, 0);
        }
        self.scroll_to_line(line);
        cx.notify();
    }

    // ---- Свёртывание (4.5) ----

    /// Клик по маркеру свёртывания в гуттере.
    pub(crate) fn toggle_fold_at(&mut self, line: usize, cx: &mut Context<Self>) {
        self.with_active_tab(|tab| tab.toggle_fold(line), cx);
    }

    /// «Свернуть всё» (меню View; без горячей клавиши — 8.5).
    pub(crate) fn on_fold_all(&mut self, _: &FoldAll, _window: &mut Window, cx: &mut Context<Self>) {
        self.with_active_tab(|tab| tab.fold_all(), cx);
    }

    /// «Развернуть всё» (меню View).
    pub(crate) fn on_unfold_all(&mut self, _: &UnfoldAll, _window: &mut Window, cx: &mut Context<Self>) {
        self.with_active_tab(|tab| tab.unfold_all(), cx);
    }

    // ---- Операции со строками (4.6): точки запуска — меню Text,
    // горячих клавиш нет (8.5) ----

    pub(crate) fn on_sort_asc(&mut self, _: &SortAsc, _w: &mut Window, cx: &mut Context<Self>) {
        self.with_active_tab(|tab| tab.sort_lines(true, false), cx);
    }

    pub(crate) fn on_sort_desc(&mut self, _: &SortDesc, _w: &mut Window, cx: &mut Context<Self>) {
        self.with_active_tab(|tab| tab.sort_lines(false, false), cx);
    }

    pub(crate) fn on_sort_asc_ignore_case(
        &mut self,
        _: &SortAscIgnoreCase,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_active_tab(|tab| tab.sort_lines(true, true), cx);
    }

    pub(crate) fn on_sort_desc_ignore_case(
        &mut self,
        _: &SortDescIgnoreCase,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_active_tab(|tab| tab.sort_lines(false, true), cx);
    }

    pub(crate) fn on_remove_duplicate_lines(
        &mut self,
        _: &RemoveDuplicateLines,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_active_tab(|tab| tab.dedupe_lines(), cx);
    }

    pub(crate) fn on_remove_empty_lines(
        &mut self,
        _: &RemoveEmptyLines,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_active_tab(|tab| tab.remove_empty_lines(), cx);
    }

    pub(crate) fn on_upper_case(&mut self, _: &UpperCase, _w: &mut Window, cx: &mut Context<Self>) {
        self.with_active_tab(|tab| tab.change_case(editor_core::CaseMode::Upper), cx);
    }

    pub(crate) fn on_lower_case(&mut self, _: &LowerCase, _w: &mut Window, cx: &mut Context<Self>) {
        self.with_active_tab(|tab| tab.change_case(editor_core::CaseMode::Lower), cx);
    }

    pub(crate) fn on_title_case(&mut self, _: &TitleCase, _w: &mut Window, cx: &mut Context<Self>) {
        self.with_active_tab(|tab| tab.change_case(editor_core::CaseMode::Title), cx);
    }

    pub(crate) fn on_sentence_case(&mut self, _: &SentenceCase, _w: &mut Window, cx: &mut Context<Self>) {
        self.with_active_tab(|tab| tab.change_case(editor_core::CaseMode::Sentence), cx);
    }

    // ---- Мульти-курсор (4.3): Cmd+D ----

    /// Следующий курсор на совпадение слова; скроллим к новому
    /// главному курсору.
    pub(crate) fn on_add_next_match(&mut self, _: &AddNextMatch, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.model_mut().active_mut()
            && tab.add_next_word_match() {
                let line = tab.cursor_line();
                self.scroll_to_line(line);
            }
        cx.notify();
    }
}
