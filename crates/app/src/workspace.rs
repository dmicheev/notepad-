//! Workspace — корневой view приложения: сборка layout (левая панель
//! «Project» — project_panel.rs, таб-бар — tab_bar.rs, редактор —
//! editor_view.rs, статус-бар — status_bar.rs, overlay Cmd+G —
//! goto_dialog.rs), кэш мапы видимых строк (folding) и свап
//! scroll-позиций вкладок, сессия (persist/session_state).
//!
//! Редактирование — только через tab_model → editor-core
//! (Document::apply + History); курсоры смещаются правками через
//! map_selection. Обработчики действий — editor_actions.rs, мышь —
//! editor_mouse.rs, геометрия — geometry.rs.

use gpui::{
    Context, FocusHandle, Window, div, point, prelude::*, px, rgb, UniformListScrollHandle,
};

use crate::actions::Quit;
use crate::editor_mouse::DragState;
use crate::session::{self, SessionState, WindowGeometry};
use crate::tab_model::TabModel;
use crate::theme::*;

/// Корневой view приложения.
pub struct Workspace {
    pub model: TabModel,
    pub focus: FocusHandle,
    pub(crate) scroll: UniformListScrollHandle,
    /// Транзиентное сообщение статус-бара (ошибки открытия/сохранения,
    /// пропущенные при рестарте вкладки); живёт до следующего сообщения.
    pub status_message: Option<String>,
    /// Кэш заголовка окна (обновляем в render при изменении).
    pub(crate) rendered_title: Option<String>,
    /// Индекс вкладки, чей scroll сейчас в `scroll` (свап при
    /// переключении вкладок; после закрытия вкладки индекс может
    /// указывать на соседку — допустимо для MVP, самокорректируется).
    pub(crate) scroll_owner: Option<usize>,
    /// Мышь в редакторе: активный драг/клик (4.2).
    pub(crate) drag: Option<DragState>,
    /// Ширина char в px моноширинного шрифта, измеряется один раз через
    /// `window.text_system()` (4.2/D9.2).
    pub(crate) char_advance: Option<f32>,
    /// Диалог перехода к строке (Cmd+G, 4.2): буфер ввода цифр.
    /// Overlay внутри окна (без нового окна — D9.2).
    pub goto_input: Option<String>,
    /// Фокус-хендл overlay-ввода (клавиши диалога не попадают в редактор).
    pub(crate) goto_focus: FocusHandle,
    /// Кэш мапы visible→real для uniform_list (4.5/D9.5): (индекс
    /// вкладки, версия вида, мапа). Пересчитывается при правках,
    /// фолдах и переключении вкладок (ключ — view_version).
    pub(crate) fold_view: Option<(usize, u64, Vec<usize>)>,
}

impl Workspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Workspace {
            model: TabModel::new(),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            status_message: None,
            rendered_title: None,
            scroll_owner: None,
            drag: None,
            char_advance: None,
            goto_input: None,
            goto_focus: cx.focus_handle(),
            fold_view: None,
        }
    }

    /// Пересчитать мапу видимых строк, если устарела. Вызывается перед
    /// любым использованием мапы (рендер, координаты мыши, скролл).
    pub(crate) fn ensure_fold_view(&mut self) {
        let key = self.model.active_index();
        let need = match (key, &self.fold_view) {
            (Some(ix), Some((cached_ix, version, _))) => {
                self.model.tabs()[ix].view_version() != *version || *cached_ix != ix
            }
            (Some(_), None) => true,
            (None, _) => false,
        };
        if need {
            if let Some(ix) = key {
                let tab = &self.model.tabs()[ix];
                let map = crate::folding::visible_lines(&tab.doc);
                let version = tab.view_version();
                self.fold_view = Some((ix, version, map));
            }
        }
        if key.is_none() {
            self.fold_view = None;
        }
    }

    /// Мапа visible→real активной вкладки (после ensure_fold_view).
    fn fold_map(&self) -> &[usize] {
        self.fold_view.as_ref().map(|(_, _, m)| m.as_slice()).unwrap_or(&[])
    }

    // Геометрия (editor_bounds / char_advance / visible_line_at_y /
    // col_at_x / page_lines) — в geometry.rs, единый источник (B).

    /// Индекс uniform_list (видимый) → real-строка документа (мапа
    /// folding, D9.5). Скрытых индексов не бывает.
    pub(crate) fn visible_to_real(&self, ix: usize) -> usize {
        let map = self.fold_map();
        map.get(ix).copied().unwrap_or(ix)
    }

    /// Сколько элементов сейчас в uniform_list (видимых строк).
    pub(crate) fn visible_len(&self) -> usize {
        self.fold_map().len()
    }

    /// Прокрутить к real-строке (через мапу видимых), стратегия —
    /// центр вьюпорта. Скрытая строка сначала поднимается к заголовку
    /// фолда (D9.5).
    pub(crate) fn scroll_to_line(&mut self, line: usize) {
        self.ensure_fold_view();
        let line = self
            .model
            .active()
            .map(|t| t.visible_line_or_header(line))
            .unwrap_or(line);
        let vis = self.real_to_visible(line);
        self.scroll.scroll_to_item(vis, gpui::ScrollStrategy::Center);
    }

    /// real-строка → индекс uniform_list (видимый); скрытая строка —
    /// её заголовок.
    fn real_to_visible(&self, line: usize) -> usize {
        let line = self
            .model
            .active()
            .map(|t| t.visible_line_or_header(line))
            .unwrap_or(line);
        crate::folding::real_to_visible(self.fold_map(), line)
            .unwrap_or_else(|| self.fold_map().len().saturating_sub(1))
    }

    // Мышь в редакторе (4.2/4.3): DragState, pos_to_line_col и
    // обработчики down/move/up — в editor_mouse.rs.

    // ---- Сессия (3.4) ----

    /// Собрать состояние сессии из модели: только вкладки с путями
    /// (untitled не восстанавливаются — D8.4), активная — пересчитанный
    /// индекс среди сохранённых.
    pub fn session_state(&self, geometry: Option<WindowGeometry>) -> SessionState {
        let mut tabs = Vec::new();
        let mut active_tab = None;
        for (i, t) in self.model.tabs().iter().enumerate() {
            let Some(path) = &t.path else { continue };
            if self.model.active_index() == Some(i) {
                active_tab = Some(tabs.len());
            }
            tabs.push(session::SessionTab { path: path.clone() });
        }
        SessionState {
            workspace: None, // воркспейс — группа 7
            tabs,
            active_tab,
            window: geometry,
        }
    }

    /// Сохранить сессию (синхронно; файл маленький) с текущей геометрией
    /// окна. Вызывается из on_window_should_close и on_app_quit.
    pub fn persist(&self, window: &Window) {
        let bounds = match window.window_bounds() {
            gpui::WindowBounds::Windowed(b)
            | gpui::WindowBounds::Maximized(b)
            | gpui::WindowBounds::Fullscreen(b) => b,
        };
        let geometry = WindowGeometry {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
        };
        let state = self.session_state(Some(geometry));
        match session::session_path() {
            Some(path) => {
                if let Err(e) = session::save_session(&state, &path) {
                    eprintln!("[session] save failed: {e}");
                }
            }
            None => eprintln!("[session] data dir unavailable, session not saved"),
        }
    }
}

impl gpui::Focusable for Workspace {
    fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Мапа видимых строк (folding, D9.5) — пересчёт при устаревании.
        self.ensure_fold_view();

        // Заголовок окна = активная вкладка (кэш — set только при
        // изменении, чтобы не перерисовывать окно каждый кадр).
        let desired_title = self
            .model
            .active_index()
            .and_then(|i| self.model.title(i))
            .unwrap_or_else(|| "notepad+".to_string());
        if self.rendered_title.as_deref() != Some(desired_title.as_str()) {
            window.set_window_title(&desired_title);
            self.rendered_title = Some(desired_title);
        }

        // Свап scroll-позиции при переключении вкладки (D8.4): уходящая
        // вкладка запоминает текущее смещение, входящая восстанавливает.
        let active_ix = self.model.active_index();
        if self.scroll_owner != active_ix {
            if let Some(owner) = self.scroll_owner {
                let y = f32::from(self.scroll.0.borrow().base_handle.offset().y);
                if let Some(tab) = self.model.tab_mut(owner) {
                    tab.scroll_y = y;
                }
            }
            if let Some(ix) = active_ix {
                let y = self.model.tabs()[ix].scroll_y;
                self.scroll
                    .0
                    .borrow()
                    .base_handle
                    .set_offset(point(px(0.0), px(y)));
            }
            self.scroll_owner = active_ix;
        }

        // Левая панель и кнопки перестановки — свои модули.
        let project_panel = self.render_project_panel(cx);
        let reorder_buttons = self.render_reorder_buttons(cx);

        // Центральная колонка: таб-бар + редактор.
        let center = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .bg(rgb(TABBAR))
                    .child(self.render_tab_bar(cx).flex_1().min_w_0())
                    .child(reorder_buttons),
            )
            .child(self.render_editor(cx));

        // Корень: [ панель | центр ] по горизонтали + статус-бар внизу.
        // Действия вешаем на корень (а не на редактор): работают и при
        // пустом списке вкладок (фокус в редакторе-заглушке).
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .on_action(cx.listener(Self::on_open))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_save_as))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_go_to_line))
            .on_action(cx.listener(Self::on_add_next_match))
            .on_action(cx.listener(Self::on_toggle_bookmark))
            .on_action(cx.listener(Self::on_next_bookmark))
            .on_action(cx.listener(Self::on_prev_bookmark))
            .on_action(cx.listener(Self::on_fold_all))
            .on_action(cx.listener(Self::on_unfold_all))
            .on_action(cx.listener(Self::on_sort_asc))
            .on_action(cx.listener(Self::on_sort_desc))
            .on_action(cx.listener(Self::on_sort_asc_ignore_case))
            .on_action(cx.listener(Self::on_sort_desc_ignore_case))
            .on_action(cx.listener(Self::on_remove_duplicate_lines))
            .on_action(cx.listener(Self::on_remove_empty_lines))
            .on_action(cx.listener(Self::on_upper_case))
            .on_action(cx.listener(Self::on_lower_case))
            .on_action(cx.listener(Self::on_title_case))
            .on_action(cx.listener(Self::on_sentence_case))
            .on_action(cx.listener(|_this: &mut Self, _: &Quit, _window, cx| {
                cx.notify();
                cx.quit();
            }))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(project_panel)
                    .child(center),
            )
            .child(self.render_status_bar());
        if self.goto_input.is_some() {
            root = root.child(self.render_goto_overlay(cx));
        }
        root
    }
}
