//! Панель split view (8.1/D15.1): независимый набор вкладок
//! (`TabModel`) + собственное view-состояние редактора — scroll-
//! handles, владелец scroll-позиции, кэш мапы видимых строк (фолды),
//! драг выделения и фокус. До 4 панелей (`Workspace::MAX_PANES`);
//! раскладка: 1 — целиком, 2 — строка, 3/4 — сетка 2×2.
//!
//! Активная панель (`Workspace::active_pane`) получает действия меню,
//! поиск и ввод с клавиатуры; клик по панели активирует её (паттерн
//! VS Code). Синхронная прокрутка (`sync_scroll`) — поверх позиций
//! панелей, состояние в Workspace.

use gpui::{Context, FocusHandle, UniformListScrollHandle, Window};

use crate::buffer_tab::BufferTab;
use crate::editor_mouse::DragState;
use crate::tab_model::TabModel;
use crate::workspace::{FoldView, Workspace};

/// Панель split view: вкладки + view-состояние редактора.
pub(crate) struct Pane {
    /// Независимые вкладки панели (8.1: «независимые вкладки в
    /// панелях» — VS Code-модель групп редактора).
    pub model: TabModel,
    /// Вертикальный scroll редактора панели (uniform_list).
    pub scroll: UniformListScrollHandle,
    /// Горизонтальный scroll редактора панели.
    pub h_scroll: gpui::ScrollHandle,
    /// Индекс вкладки-владельца `scroll` (свап при переключении).
    pub(crate) scroll_owner: Option<usize>,
    /// Кэш мапы видимых строк панели (folding, D9.5).
    pub(crate) fold_view: Option<(usize, FoldView)>,
    /// Драг выделения в редакторе панели (4.2).
    pub(crate) drag: Option<DragState>,
    /// Фокус редактора панели: plain-клавиши идут в ФОКУСИРОВАННУЮ
    /// панель (не обязательно последнюю активную — паттерн VS Code).
    pub focus: FocusHandle,
    /// Горизонтальный скролл таб-бара панели.
    pub tab_scroll: gpui::ScrollHandle,
}

impl Pane {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        Pane {
            model: TabModel::new(),
            scroll: UniformListScrollHandle::new(),
            h_scroll: gpui::ScrollHandle::new(),
            scroll_owner: None,
            fold_view: None,
            drag: None,
            focus: cx.focus_handle(),
            tab_scroll: gpui::ScrollHandle::new(),
        }
    }
}

impl Workspace {
    /// Активная панель (не пуста: `panes` всегда ≥ 1).
    pub(crate) fn pane(&self) -> &Pane {
        &self.panes[self.active_pane]
    }

    /// Модель вкладок активной панели (совместимый аксессор для
    /// модулей, работающих с активной панелью).
    pub(crate) fn model(&self) -> &TabModel {
        &self.pane().model
    }

    /// Мутабельная модель активной панели.
    pub(crate) fn model_mut(&mut self) -> &mut TabModel {
        &mut self.pane_mut().model
    }

    /// Мутабельная активная панель.
    pub(crate) fn pane_mut(&mut self) -> &mut Pane {
        &mut self.panes[self.active_pane]
    }

    /// Фокус активной панели (возврат фокуса из диалогов/поиска).
    pub(crate) fn active_focus(&self) -> FocusHandle {
        self.panes[self.active_pane].focus.clone()
    }

    /// Активировать панель (клик/вкладка/действие): активная получает
    /// меню-действия; поиск File-scope пересчитывается по новой
    /// активной вкладке (совпадения привязаны к документу).
    pub(crate) fn activate_pane(&mut self, pi: usize, cx: &mut Context<Self>) {
        if pi < self.panes.len() && self.active_pane != pi {
            self.active_pane = pi;
            if self.search.as_ref().is_some_and(|sp| sp.scope == crate::search_panel::SearchScope::File) {
                self.search_recompute_file(cx);
            }
            cx.notify();
        }
    }

    /// Активировать панель и вернуть фокус её редактору (клики).
    pub(crate) fn focus_pane(&mut self, pi: usize, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.activate_pane(pi, cx);
        if pi < self.panes.len() {
            let f = self.panes[pi].focus.clone();
            window.focus(&f);
        }
    }

    // ---- Split view: операции (8.1) ----

    /// Разделить редактор (Cmd+\): новая пустая панель справа/
    /// снизу-справа (сетка 2×2 при 3–4), активируется. Максимум
    /// `MAX_PANES` — no-op. Новая панель пуста: файлы открываются в
    /// фокусированную панель (Cmd+O после split).
    pub(crate) fn split_pane(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if self.panes.len() >= Self::MAX_PANES {
            return;
        }
        let pi = self.panes.len();
        self.panes.push(Pane::new(cx));
        self.active_pane = pi;
        window.focus(&self.panes[pi].focus);
        cx.notify();
    }

    /// Закрыть активную панель (Cmd+Shift+\): dirty-вкладки —
    /// последовательный диалог Save / Don't Save / Cancel (отмена
    /// останавливает закрытие), чистая панель закрывается сразу.
    /// Последняя панель не закрывается.
    pub(crate) fn close_active_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.panes.len() <= 1 {
            return;
        }
        let pi = self.active_pane;
        // Первая dirty-вкладка панели (ascending).
        let dirty = (0..self.panes[pi].model.len())
            .find(|&ix| self.panes[pi].model.tabs()[ix].dirty);
        match dirty {
            None => self.remove_pane(pi, cx),
            Some(ix) => {
                let title = self.panes[pi]
                    .model
                    .title(ix)
                    .unwrap_or_else(|| "Untitled".into());
                let rx = window.prompt(
                    gpui::PromptLevel::Warning,
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
                    this.update_in(cx, |ws, window, cx| match choice {
                        0 => ws.save_pane_tab_then(pi, ix, window, cx),
                        1 => {
                            if let Some(pane) = ws.panes.get_mut(pi) {
                                pane.model.close(ix);
                            }
                            ws.close_active_pane_at(pi, window, cx);
                        }
                        _ => {}
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    /// Продолжение закрытия панели pi после диалога (следующая
    /// dirty-вкладка или удаление панели).
    fn close_active_pane_at(&mut self, pi: usize, window: &mut Window, cx: &mut Context<Self>) {
        if pi >= self.panes.len() {
            return;
        }
        let dirty = (0..self.panes[pi].model.len())
            .find(|&ix| self.panes[pi].model.tabs()[ix].dirty);
        match dirty {
            None => self.remove_pane(pi, cx),
            Some(ix) => {
                let title = self.panes[pi]
                    .model
                    .title(ix)
                    .unwrap_or_else(|| "Untitled".into());
                let rx = window.prompt(
                    gpui::PromptLevel::Warning,
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
                    this.update_in(cx, |ws, window, cx| match choice {
                        0 => ws.save_pane_tab_then(pi, ix, window, cx),
                        1 => {
                            if let Some(pane) = ws.panes.get_mut(pi) {
                                pane.model.close(ix);
                            }
                            ws.close_active_pane_at(pi, window, cx);
                        }
                        _ => {}
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    /// Сохранить вкладку (untitled — save-as диалог), затем закрыть и
    /// продолжить закрытие панели. Отмена диалога оставляет вкладку —
    /// закрытие панели останавливается.
    fn save_pane_tab_then(&mut self, pi: usize, ix: usize, _window: &mut Window, cx: &mut Context<Self>) {
        let has_path = self
            .panes
            .get(pi)
            .and_then(|p| p.model.tabs().get(ix))
            .is_some_and(|t| t.path.is_some());
        if !has_path {
            // Untitled: save-as; после исхода — повторить попытку
            // закрытия панели (паттерн save_as_then_close D8.2).
            let dir = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
            let rx = cx.prompt_for_new_path(&dir, Some("Untitled.txt"));
            cx.spawn(async move |this, cx| {
                if let Ok(Ok(Some(path))) = rx.await {
                    this.update(cx, |ws, cx| {
                        let saved = ws
                            .panes
                            .get_mut(pi)
                            .and_then(|p| p.model.tab_mut(ix))
                            .is_some_and(|t| t.save_to(&path).is_ok());
                        if saved
                            && let Some(pane) = ws.panes.get_mut(pi)
                        {
                            pane.model.close(ix);
                        }
                        cx.notify();
                    })
                    .ok();
                }
            })
            .detach();
            return;
        }
        let saved = self
            .panes
            .get_mut(pi)
            .and_then(|p| p.model.tab_mut(ix))
            .map(|t| t.save().is_ok())
            .unwrap_or(false);
        if saved
            && let Some(pane) = self.panes.get_mut(pi)
        {
            pane.model.close(ix);
        }
        // Продолжить закрытие (следующий диалог/удаление) нельзя звать
        // с window здесь после prompt-цепочки — закрываем без него:
        // оставшиеся dirty всё ещё спросят при следующем вызове.
        if !self
            .panes
            .get(pi)
            .is_some_and(|p| p.model.tabs().iter().any(|t| t.dirty))
        {
            self.remove_pane(pi, cx);
        }
        cx.notify();
    }

    /// Удалить панель (все вкладки закрыты); активной становится
    /// соседняя слева (крайняя левая — правая).
    fn remove_pane(&mut self, pi: usize, cx: &mut Context<Self>) {
        if self.panes.len() <= 1 {
            return;
        }
        self.panes.remove(pi);
        self.active_pane = self.active_pane.min(self.panes.len() - 1);
        cx.notify();
    }

    /// Тумблер синхронной прокрутки (меню View): прокрутка любой
    /// панели прокручивает остальные по первой видимой строке.
    pub(crate) fn toggle_sync_scroll(&mut self, cx: &mut Context<Self>) {
        self.sync_scroll = !self.sync_scroll;
        self.set_status_message(if self.sync_scroll {
            crate::i18n::t(cx, "status.sync_scroll_on")
        } else {
            crate::i18n::t(cx, "status.sync_scroll_off")
        }, cx);
        cx.notify();
    }

    /// Синхронная прокрутка (8.1): выровнять остальные панели по
    /// первой видимой строке панели `pi` (строчная шкала; фолды в
    /// других панелях дают приблизительное соответствие — MVP).
    /// Вызывается после интерактивной прокрутки (колесо/ползунок).
    pub(crate) fn sync_scroll_from(&mut self, pi: usize, cx: &mut Context<Self>) {
        if !self.sync_scroll || pi >= self.panes.len() {
            return;
        }
        let offset_y = f32::from(self.panes[pi].scroll.0.borrow().base_handle.offset().y);
        let top = ((-offset_y).max(0.0) / self.line_height()).floor() as usize;
        let n = self.panes.len();
        for pj in 0..n {
            if pj == pi {
                continue;
            }
            let len = self.panes[pj]
                .model
                .active()
                .map(|t| t.doc.len_lines())
                .unwrap_or(0);
            if len == 0 {
                continue;
            }
            let target = top.min(len - 1);
            self.panes[pj]
                .scroll
                .scroll_to_item(target, gpui::ScrollStrategy::Top);
        }
        cx.notify();
    }

    /// Перебросить вкладки вслед за переименованным/перемещённым
    /// файлом во ВСЕХ панелях (7.2 поверх split view).
    pub(crate) fn retag_path_all(&mut self, old: &std::path::Path, new: &std::path::Path) -> bool {
        let mut hit = false;
        for pane in &mut self.panes {
            hit |= pane.model.retag_path(old, new);
        }
        hit
    }

    /// Все открытые вкладки всех панелей: (панель, индекс) — поиск по
    /// вкладкам/папке (6.3/6.4) и прочие обходы поверх split view.
    pub(crate) fn all_tabs(&self) -> impl Iterator<Item = (usize, usize, &BufferTab)> {
        self.panes
            .iter()
            .enumerate()
            .flat_map(|(pi, pane)| {
                pane.model
                    .tabs()
                    .iter()
                    .enumerate()
                    .map(move |(ix, tab)| (pi, ix, tab))
            })
    }
}
