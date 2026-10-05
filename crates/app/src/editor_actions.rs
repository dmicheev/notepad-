//! Обработчики действий (меню/горячие клавиши) и клавиатурный ввод
//! редактора (группы 3–4): файлы (open/save/close), undo/redo,
//! закладки, свёртывание, операции со строками, Cmd+D. Регистрация
//! on_action — на корне (workspace.rs::render); типы действий —
//! actions.rs; биндинги — main.rs.

use std::path::PathBuf;

use gpui::{Context, KeyDownEvent, PathPromptOptions, PromptLevel, Window};

use crate::actions::*;
use crate::workspace::Workspace;

impl Workspace {
    // ---- Действия (минимальный набор группы 3; полный справочник — 8.5) ----

    /// Cmd+O: диалог ОС (gpui 0.2.2 `prompt_for_paths`; rfd не нужен —
    /// D8.3), открытие каждого файла обычной вкладкой.
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
        let mut first_error: Option<String> = None;
        let mut opened: Vec<usize> = Vec::new();
        for path in paths {
            match self.model.open_path(path) {
                Ok(i) => opened.push(i),
                Err(e) => {
                    let msg = format!("Could not open {}: {e}", path.display());
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
                self.model
                    .tab_mut(i)
                    .is_some_and(|t| t.language.over_threshold && !t.language.threshold_allowed)
            })
            .collect();
        if !over.is_empty() && !crate::bench::enabled() {
            self.pending_threshold_tabs.extend(over);
            self.ask_pending_threshold(window, cx);
        }
        // Фоновая раскраска новых вкладок чекпоинтами (сразу, не
        // дожидаясь прыжков пользователя; workspace.rs).
        for i in &opened {
            self.spawn_background_highlight(*i, cx);
        }
        cx.notify();
    }

    /// Cmd+S: сохранить активную вкладку; untitled → save-as диалог.
    pub(crate) fn on_save(&mut self, _: &SaveFile, _window: &mut Window, cx: &mut Context<Self>) {
        let has_path = self
            .model
            .active()
            .is_some_and(|t| t.path.is_some());
        if has_path {
            self.save_active(cx);
        } else if self.model.active().is_some() {
            self.save_as(cx);
        }
    }

    /// Cmd+Shift+S: сохранить как (в т.ч. первый Cmd+S для untitled).
    pub(crate) fn on_save_as(&mut self, _: &SaveFileAs, _window: &mut Window, cx: &mut Context<Self>) {
        if self.model.active().is_some() {
            self.save_as(cx);
        }
    }

    /// Сохранение по существующему пути; ошибка — в статус-бар.
    fn save_active(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.model.active_mut()
            && let Err(e) = tab.save() {
                self.status_message = Some(format!("Save failed: {e}"));
            }
        cx.notify();
    }

    /// Диалог сохранения (gpui `prompt_for_new_path`, D8.3).
    fn save_as(&mut self, cx: &mut Context<Self>) {
        let dir = self
            .model
            .active()
            .and_then(|t| t.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())))
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let rx = cx.prompt_for_new_path(&dir, Some("Untitled.txt"));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                this.update(cx, |ws, cx| {
                    if let Some(tab) = ws.model.active_mut()
                        && let Err(e) = tab.save_to(&path) {
                            ws.status_message = Some(format!("Save failed: {e}"));
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
        if let Some(index) = self.model.active_index() {
            self.close_tab_at(index, window, cx);
        }
    }

    pub(crate) fn close_tab_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let dirty = self.model.tabs().get(index).is_some_and(|t| t.dirty);
        if !dirty {
            self.model.close(index);
            cx.notify();
            return;
        }
        let title = self.model.title(index).unwrap_or_else(|| "Untitled".into());
        let rx = window.prompt(
            PromptLevel::Warning,
            &format!("Do you want to save the changes you made in \"{title}\"?"),
            Some("Your changes will be lost if you don't save them."),
            &["Save", "Don't Save", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = rx.await else { return };
            this.update_in(cx, |ws, _window, cx| match choice {
                0 => ws.save_and_close(index, cx),
                1 => {
                    ws.model.close(index);
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
    fn save_and_close(&mut self, index: usize, cx: &mut Context<Self>) {
        let has_path = self.model.tabs().get(index).is_some_and(|t| t.path.is_some());
        if !has_path {
            self.save_as_then_close(index, cx);
            return;
        }
        match self.model.tab_mut(index).map(|t| t.save()) {
            Some(Ok(())) => {
                self.model.close(index);
                cx.notify();
            }
            Some(Err(e)) => self.status_message = Some(format!("Save failed: {e}")),
            None => {}
        }
    }

    fn save_as_then_close(&mut self, index: usize, cx: &mut Context<Self>) {
        let dir = self
            .model
            .tabs()
            .get(index)
            .and_then(|t| t.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())))
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let rx = cx.prompt_for_new_path(&dir, Some("Untitled.txt"));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                this.update(cx, |ws, cx| {
                    let saved = ws
                        .model
                        .tab_mut(index)
                        .is_some_and(|t| t.save_to(&path).is_ok());
                    if saved {
                        ws.model.close(index);
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub(crate) fn on_undo(&mut self, _: &Undo, _window: &mut Window, cx: &mut Context<Self>) {
        self.with_active_tab(|tab| { tab.undo(); }, cx);
    }

    pub(crate) fn on_redo(&mut self, _: &Redo, _window: &mut Window, cx: &mut Context<Self>) {
        self.with_active_tab(|tab| { tab.redo(); }, cx);
    }

    // ---- Клавиатурный ввод редактора (plain-клавиши; Cmd/Ctrl —
    // действия выше; список клавиш — D9.2/D9.3) ----

    pub(crate) fn handle_key(
        &mut self,
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
        let extend = ks.modifiers.shift;
        let page = self.page_lines(window);
        let Some(tab) = self.model.active_mut() else { return };
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
        if let Some(tab) = self.model.active_mut()
            && tab.goto_next_bookmark() {
                let line = tab.cursor_line();
                self.scroll_to_line(line);
            }
        cx.notify();
    }

    /// Shift+F2: предыдущая закладка по кругу (+ скролл).
    pub(crate) fn on_prev_bookmark(&mut self, _: &PrevBookmark, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.model.active_mut()
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
        if let Some(tab) = self.model.active_mut() {
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
        if let Some(tab) = self.model.active_mut()
            && tab.add_next_word_match() {
                let line = tab.cursor_line();
                self.scroll_to_line(line);
            }
        cx.notify();
    }
}
