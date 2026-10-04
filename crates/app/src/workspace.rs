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

/// Кэш мапы visible→real для uniform_list (4.5/D9.5, D11.2).
///
/// Два состояния:
/// - `Identity` — фолдов нет: мапа тождественная, НЕ аллоцируется и
///   НЕ инвалидируется правками текста (правки не меняют folded-флаги
///   и видимость); число видимых строк берётся из документа напрямую.
///   Инвалидация — только появление фолдов (toggle_fold/fold_all/undo,
///   восстанавливающий folded-флаги).
/// - `Mapped` — есть фолды: материализованный Vec (O(n) памяти и
///   пересборки); ключ — view_version (правки/undo/redo сдвигают
///   строки тел — мапа обязана пересчитаться).
#[derive(Debug)]
pub(crate) enum FoldView {
    Identity,
    Mapped { version: u64, map: Vec<usize> },
}

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
    /// Overlay выбора языка (5.1): буфер фильтра; фокус — отдельный
    /// handle (паттерн goto_dialog).
    pub language_input: Option<String>,
    pub(crate) language_focus: FocusHandle,
    /// Прогрев syntect запланирован (однократно; после первого кадра —
    /// инициализация набора вне критического пути, D12.1).
    pub(crate) syntax_warmup_scheduled: bool,
    /// Вкладки сверх порога 20 МБ, ожидающие вопроса пользователю
    /// (диалог ставится, когда доступен Window — сессия/NOTEPAD_OPEN
    /// собирают до окна, D12.4).
    pub(crate) pending_threshold_tabs: Vec<usize>,
    /// Кэш мапы видимых строк (folding, D9.5/D11.2): индекс вкладки +
    /// состояние (`FoldView`). Пересчёт — по правилам `FoldView`.
    pub(crate) fold_view: Option<(usize, FoldView)>,
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
            language_input: None,
            language_focus: cx.focus_handle(),
            syntax_warmup_scheduled: false,
            pending_threshold_tabs: Vec::new(),
            fold_view: None,
        }
    }

    /// Прогрев подсветки (5.1/D12.1): инициализация набора синтаксисов
    /// (~сотни мс) + детект языков вкладок + первый разбор активной
    /// (ленивые контексты/regex'ы синтаксиса — вне рендера). Вызывается
    /// отложенно после первого кадра — холодный старт и открытие файлов
    /// syntect не платят.
    pub fn refresh_all_languages(&mut self) {
        for i in 0..self.model.len() {
            if let Some(tab) = self.model.tab_mut(i) {
                tab.refresh_language();
            }
        }
        // Прогреть грамматику активной вкладки (ленивая десериализация
        // контекстов + компиляция regex'ов — разовая цена, D12.1).
        if let Some(tab) = self.model.active_mut() {
            if tab.highlight.syntax_name().is_some() {
                let lines = tab.doc.len_lines();
                tab.highlight.ensure_window(&tab.doc, 0, lines.min(64));
            }
        }
    }

    /// Транзиентное сообщение при открытии файла сверх порога (5.3):
    /// подсветка отключена, редактирование доступно. Вызывается из
    /// точек открытия (диалог, сессия, NOTEPAD_OPEN).
    pub(crate) fn note_highlight_threshold(&mut self) {
        if let Some(tab) = self.model.active() {
            if tab.language.disabled() {
                self.status_message = Some(
                    "Syntax highlighting is off: file exceeds 20 MB".to_string(),
                );
            }
        }
    }

    /// Поставить вкладки сверх порога в очередь на вопрос (D12.4):
    /// диалог показывается, когда доступен Window (сессия/NOTEPAD_OPEN
    /// открывают вкладки до окна). Bench-режим не спрашивает.
    pub fn queue_threshold_ask(&mut self) {
        if crate::bench::enabled() {
            return;
        }
        for i in 0..self.model.len() {
            let over = self
                .model
                .tab_mut(i)
                .is_some_and(|t| t.language.over_threshold && !t.language.threshold_allowed);
            if over && !self.pending_threshold_tabs.contains(&i) {
                self.pending_threshold_tabs.push(i);
            }
        }
    }

    /// Спросить пользователя о подсветке файлов сверх порога 20 МБ
    /// (правка спеки окт 2026): согласие — подсветка любого размера
    /// (догон с бюджетом держит кадры), отказ — выключена, сообщение.
    pub fn ask_pending_threshold(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let indices = std::mem::take(&mut self.pending_threshold_tabs);
        let indices: Vec<usize> = indices
            .into_iter()
            .filter(|&i| {
                self.model
                    .tab_mut(i)
                    .is_some_and(|t| t.language.over_threshold && !t.language.threshold_allowed)
            })
            .collect();
        if indices.is_empty() || crate::bench::enabled() {
            return;
        }
        let count = indices.len();
        let single = count == 1;
        let message: std::borrow::Cow<'static, str> = if single {
            "This file is over 20 MB. Enable syntax highlighting anyway?".into()
        } else {
            format!("{count} opened files are over 20 MB. Enable syntax highlighting for them anyway?").into()
        };
        let rx = window.prompt(
            gpui::PromptLevel::Warning,
            &message,
            Some("Highlighting large files is progressive (budgeted per frame); scrolling and typing stay responsive."),
            &["Enable", "Keep off"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = rx.await else { return };
            this.update_in(cx, |ws, _window, cx| {
                match choice {
                    0 => {
                        for i in indices {
                            if let Some(tab) = ws.model.tab_mut(i) {
                                tab.allow_highlight_despite_threshold();
                            }
                        }
                        ws.status_message = Some(
                            "Syntax highlighting enabled for large file(s)".to_string(),
                        );
                    }
                    _ => {
                        ws.status_message = Some(
                            "Syntax highlighting is off: file exceeds 20 MB".to_string(),
                        );
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Пересчитать мапу видимых строк, если устарела. Вызывается перед
    /// любым использованием мапы (рендер, координаты мыши, скролл).
    ///
    /// Правила инвалидации (D11.2): тождественная мапа живёт, пока
    /// фолдов нет (правки текста folded-флаги не трогают); материальная
    /// мапа пересобирается при ЛЮБОМ изменении view_version и при
    /// уходе фолдов в ноль (тогда — обратно в тождественную, бесплатно).
    /// Undo/redo меняет folded-флаги только вместе с LineMeta-синком
    /// правок (Document::apply) — счётчик folded_count это покрывает:
    /// 0→N ловится переходом Identity→Mapped, N→0 — обратно.
    pub(crate) fn ensure_fold_view(&mut self) {
        let Some(ix) = self.model.active_index() else {
            self.fold_view = None;
            return;
        };
        let tab = &self.model.tabs()[ix];
        let folded = tab.doc.line_meta().folded_count();
        let version = tab.view_version();
        let need = match &self.fold_view {
            Some((cached_ix, view)) if *cached_ix == ix => match view {
                FoldView::Identity => folded > 0,
                FoldView::Mapped { version: v, .. } => folded == 0 || *v != version,
            },
            _ => true, // вкладки сменились / кэша нет
        };
        if need {
            self.fold_view = Some((
                ix,
                if folded == 0 {
                    FoldView::Identity
                } else {
                    FoldView::Mapped { version, map: crate::folding::visible_lines(&tab.doc) }
                },
            ));
        }
    }

    /// Индекс uniform_list (видимый) → real-строка документа (мапа
    /// folding, D9.5). Скрытых индексов не бывает; тождественная мапа —
    /// видимый индекс и есть real-строка.
    pub(crate) fn visible_to_real(&self, ix: usize) -> usize {
        match &self.fold_view {
            Some((_, FoldView::Mapped { map, .. })) => map.get(ix).copied().unwrap_or(ix),
            _ => ix,
        }
    }

    /// Сколько элементов сейчас в uniform_list (видимых строк).
    pub(crate) fn visible_len(&self) -> usize {
        match &self.fold_view {
            Some((_, FoldView::Identity)) => self
                .model
                .active()
                .map(|t| t.doc.len_lines())
                .unwrap_or(0),
            Some((_, FoldView::Mapped { map, .. })) => map.len(),
            None => 0,
        }
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
    /// её заголовок. Тождественная мапа: clamp в последнюю строку
    /// (как fallback материальной).
    fn real_to_visible(&self, line: usize) -> usize {
        let line = self
            .model
            .active()
            .map(|t| t.visible_line_or_header(line))
            .unwrap_or(line);
        match &self.fold_view {
            Some((_, FoldView::Mapped { map, .. })) => crate::folding::real_to_visible(map, line)
                .unwrap_or_else(|| map.len().saturating_sub(1)),
            _ => line.min(self.visible_len().saturating_sub(1)),
        }
    }

    // Геометрия (editor_bounds / char_advance / visible_line_at_y /
    // col_at_x / page_lines) — в geometry.rs, единый источник (B).

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
        // Бенчмарк-стенд (D11.1) и панель PERFORMANCE (D11.8): один
        // Instant-замер — CPU-стоимость render (ensure_fold_view +
        // элементное дерево), без vsync-ожидания.
        let frame_started = std::time::Instant::now();

        // Мапа видимых строк (folding, D9.5) — пересчёт при устаревании.
        self.ensure_fold_view();

        // Прогрев подсветки (D12.1): один раз, отложенно (50 мс) после
        // первого кадра — инициализация syntect и первый разбор НЕ на
        // критическом пути старта/открытия. Прогрев меняет состояние —
        // notify перерисует (язык в статус-баре, подсветка строк).
        if !self.syntax_warmup_scheduled {
            self.syntax_warmup_scheduled = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                this.update(cx, |ws, cx| {
                    ws.refresh_all_languages();
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }

        // Бенчмарк-стенд (D11.1): момент первого кадра (OnceLock —
        // только первое срабатывание).
        if crate::bench::enabled() {
            crate::bench::record_first_frame();
        }

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
            .child(self.render_status_bar(cx));
        if self.goto_input.is_some() {
            root = root.child(self.render_goto_overlay(cx));
        }
        if self.language_input.is_some() {
            root = root.child(self.render_language_overlay(cx));
        }
        let frame_ms = frame_started.elapsed().as_secs_f64() * 1000.0;
        crate::perf::note_frame(frame_ms);
        if crate::bench::enabled() {
            crate::bench::record_render(frame_ms);
        }
        root
    }
}
