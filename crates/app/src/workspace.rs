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
    /// Панель поиска/замены (6.1–6.5, D13): состояние + результаты;
    /// фокус — отдельный handle (ловушка D9.2 — клавиши панели не
    /// доходят до handle_key редактора).
    pub search: Option<crate::search_panel::SearchPanel>,
    pub(crate) search_focus: FocusHandle,
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
    /// Дерево воркспейса (группа 7): None — воркспейс не открыт.
    pub tree: Option<crate::fs_tree::FsTree>,
    /// Overlay-диалог ввода имени дерева (7.2): NewFile/NewFolder/
    /// Rename; фокус — отдельный handle (паттерн goto-диалога).
    pub tree_dialog: Option<crate::tree_panel::TreeDialog>,
    pub(crate) tree_focus: FocusHandle,
    /// Контекстное меню дерева (7.2): узел + позиция правого клика.
    pub(crate) context_menu: Option<crate::tree_panel::TreeContextMenu>,
    /// Папка под курсором при drag-and-drop дерева (подсветка).
    pub(crate) drop_target: Option<std::path::PathBuf>,
    /// Наблюдатель FS воркспейса (7.3) + поколение (замена воркспейса
    /// останавливает поллинг старого поколения).
    pub(crate) fs_watch: Option<crate::fs_watch::FsWatch>,
    pub(crate) watch_generation: u64,
    /// Полосы прокрутки (scrollbar.rs): handle левой панели, оверлея
    /// языка и таб-бара (редактор — `scroll` выше); активный драг
    /// ползунка (move/up слушают контейнеры зон).
    pub(crate) panel_scroll: gpui::ScrollHandle,
    pub(crate) language_scroll: gpui::ScrollHandle,
    pub(crate) tab_scroll: gpui::ScrollHandle,
    pub(crate) thumb_drag: Option<crate::scrollbar::ThumbDrag>,
    /// Горизонтальный скролл редактора (внешняя зона вокруг
    /// uniform_list, editor_view.rs; вертикаль — в `scroll`).
    pub(crate) h_scroll: gpui::ScrollHandle,
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
            search: None,
            search_focus: cx.focus_handle(),
            syntax_warmup_scheduled: false,
            pending_threshold_tabs: Vec::new(),
            fold_view: None,
            tree: None,
            tree_dialog: None,
            tree_focus: cx.focus_handle(),
            context_menu: None,
            drop_target: None,
            fs_watch: None,
            watch_generation: 0,
            panel_scroll: gpui::ScrollHandle::new(),
            language_scroll: gpui::ScrollHandle::new(),
            tab_scroll: gpui::ScrollHandle::new(),
            thumb_drag: None,
            h_scroll: gpui::ScrollHandle::new(),
        }
    }

    /// Прогрев подсветки (5.1/D12.1): инициализация набора синтаксисов
    /// (~сотни мс) + детект языков вкладок + первый разбор активной
    /// (ленивые контексты/regex'ы синтаксиса — вне рендера). Вызывается
    /// отложенно после первого кадра — холодный старт и открытие файлов
    /// syntect не платят.
    pub fn refresh_all_languages(&mut self, cx: &mut Context<Self>) {
        for i in 0..self.model.len() {
            if let Some(tab) = self.model.tab_mut(i) {
                tab.refresh_language();
            }
        }
        // Прогреть грамматику активной вкладки (ленивая десериализация
        // контекстов + компиляция regex'ов — разовая цена, D12.1).
        if let Some(tab) = self.model.active_mut()
            && tab.highlight.syntax_name().is_some() {
                let lines = tab.doc.len_lines();
                tab.highlight.ensure_window(&tab.doc, 0, lines.min(64));
            }
        // Фоновый проход чекпоинтами — сразу, не ждать пользовательских
        // прыжков (большой файл красится сам, прыжок вглубь дёшев).
        for i in 0..self.model.len() {
            self.spawn_background_highlight(i, cx);
        }
    }

    /// Фоновая раскраска файла чекпоинтами (BackgroundWalk) — ходка
    /// ВЕСЬ файл сразу после открытия, не дожидаясь пользовательских
    /// прыжков: после неё любой прыжок = ≤ CHECKPOINT_EVERY строк
    /// кадрового догонa. Тактики: syntect/onig-состояния НЕ Send —
    /// фоновой поток невозможен; ходка идёт НА ГЛАВНОМ потоке таймером
    /// (тик 25 мс, бюджет 5 мс — между кадрами, UI не рвётся). Правка
    /// НИЖЕ курсора ходки безвредна (rope актуален, стек границы
    /// валиден); правка ВЫШЕ (bg_valid_upto < line) — ходка
    /// перезапускается от дальнего валидного чекпоинта движка после
    /// паузы (1 с тишины). Смена языка/закрытие вкладки — стоп.
    /// Мелкие файлы (≤ 2 чекпоинтов) не ходим — кадровый
    /// ensure_window дешевле.
    pub(crate) fn spawn_background_highlight(&mut self, ix: usize, cx: &mut Context<Self>) {
        /// Тик ходки (главный поток, между кадрами).
        const TICK: std::time::Duration = std::time::Duration::from_millis(25);
        /// CPU-бюджет порции на тик — больше кадра не занимать.
        const TICK_BUDGET: std::time::Duration = std::time::Duration::from_millis(5);
        /// Пауза перед перезапуском ходки после правок (тишина ввода).
        const RESTART_PAUSE: std::time::Duration = std::time::Duration::from_secs(1);
        let Some(tab) = self.model.tab_mut(ix) else { return };
        if tab.bg_walk_started {
            return;
        }
        // Нет синтаксиса (ещё не прогрет/детект отложен) или CSV —
        // ходки нет; ФЛАГ не ставим: язык появится (прогрев/детект/
        // allow-порог) — следующая точка входа запустит ходку.
        // (Ловушка первой итерации: open_paths зовёт раньше детекта.)
        if editor_core::highlight::BackgroundWalk::start(&tab.highlight).is_none() {
            return;
        }
        // Мелкий файл — кадровый ensure_window дешевле ходки; флаг не
        // ставим (файл может вырасти правками — периодический rescan
        // не завели, редкая ситуация).
        if tab.doc.len_lines() <= editor_core::highlight::CHECKPOINT_EVERY * 2 {
            return;
        }
        tab.bg_walk_started = true;
        tab.bg_valid_upto = usize::MAX;
        let expected_syntax = tab.highlight.syntax_name().map(str::to_string);
        cx.spawn(async move |this, cx| {
            // Ходка создаётся/пересоздаётся от прогресса движка.
            let mut walk: Option<editor_core::highlight::BackgroundWalk> = None;
            loop {
                cx.background_executor().timer(TICK).await;
                // Что делать дальше — решает замыкание на модели
                /// (правки/язык/закрытие видны только там).
                enum Step {
                    Run { done: bool },
                    Dead,
                }
                let step = this
                    .update(cx, |ws: &mut Self, cx| {
                    let Some(tab) = ws.model.tab_mut(ix) else {
                        return Step::Dead; // вкладка закрыта
                    };
                    if expected_syntax.as_deref() != tab.highlight.syntax_name() {
                        return Step::Dead; // язык сменён
                    }
                    // Правка выше курсора — стейт ходки мусор: сброс и
                    // рестарт от дальнего ВАЛИДНОГО чекпоинта движка
                    // (invalidate_from уже выкинул старые).
                    if walk.as_ref().is_some_and(|w| tab.bg_valid_upto < w.line()) {
                        walk = None;
                    }
                    if walk.is_none() {
                        walk = editor_core::highlight::BackgroundWalk::start(&tab.highlight);
                        if walk.is_some() {
                            tab.bg_valid_upto = usize::MAX; // новая эпоха
                        } else {
                            return Step::Dead;
                        }
                    }
                    let chunk = walk.as_mut().unwrap().chunk(&tab.doc, TICK_BUDGET);
                    let done = chunk.done;
                    let valid = tab.bg_valid_upto;
                    tab.highlight.merge_background(chunk, valid);
                    // Порция могла закрыть ВИДИМЫЕ строки — кадр.
                    cx.notify();
                    Step::Run { done }
                })
                .unwrap_or(Step::Dead);
                match step {
                    Step::Dead => return,
                    Step::Run { done: true } => return,
                    Step::Run { done: false } => {} // идём тиками дальше
                }
                let _ = RESTART_PAUSE; // рестарт ленивый: walk=None
                                       // пересоздаётся следующим тиком.
            }
        })
        .detach();
    }

    /// Транзиентное сообщение при открытии файла сверх порога (5.3):
    /// подсветка отключена, редактирование доступно. Вызывается из
    /// точек открытия (диалог, сессия, NOTEPAD_OPEN).
    #[allow(dead_code)] // уведомление порога живёт в ask_pending_threshold (D12.4);
                        // метод оставлен как API точек открытия (паттерн D10.4)
    pub(crate) fn note_highlight_threshold(&mut self) {
        if let Some(tab) = self.model.active()
            && tab.language.disabled() {
                self.status_message = Some(
                    "Syntax highlighting is off: file exceeds 20 MB".to_string(),
                );
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
                                // Язык включён — фоновая ходка чекпоинтами
                                // (пропущенная на открытии: синтаксиса не было).
                                ws.spawn_background_highlight(i, cx);
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

    /// Применить операцию к активной вкладке (вкладок нет — no-op) и
    /// перерисовать. Схлопывает повтор `if let Some(tab) = ...;
    /// cx.notify()` обработчиков действий (editor_actions.rs).
    pub(crate) fn with_active_tab(
        &mut self,
        op: impl FnOnce(&mut crate::buffer_tab::BufferTab),
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.model.active_mut() {
            op(tab);
        }
        cx.notify();
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

    /// Немедленная прокрутка к real-строке с центрированием (переходы
    /// поиска 6.1–6.4): ловушка deferred-скролла D12.5 —
    /// `scroll_to_item` применяется в prepaint ПОСЛЕ render, кадр после
    /// клика по ‹/› строится по СТАРОМУ offset и в живом окне «догонял»
    /// только следующим событием. Ставим целевой offset в общий Rc
    /// (тот же, что читает render) НЕМЕДЛЕННО + `scroll_to_item` тем же
    /// индексом — deferred подтвердит значение (паттерн клика по
    /// липкой шапке D12.5).
    pub(crate) fn scroll_to_line_now(&mut self, line: usize) {
        self.ensure_fold_view();
        let line = self
            .model
            .active()
            .map(|t| t.visible_line_or_header(line))
            .unwrap_or(line);
        let vis = self.real_to_visible(line);
        let viewport_h = self
            .editor_bounds()
            .map(|b| f32::from(b.size.height))
            .unwrap_or(0.0);
        if viewport_h <= 0.0 {
            // Редактор ещё не отрисован — только deferred-путь.
            self.scroll.scroll_to_item(vis, gpui::ScrollStrategy::Center);
            return;
        }
        let content_h = self.visible_len() as f32 * crate::geometry::LINE_HEIGHT;
        // Центр вьюпорта на середине строки (семантика Center).
        let target = vis as f32 * crate::geometry::LINE_HEIGHT
            + crate::geometry::LINE_HEIGHT / 2.0
            - viewport_h / 2.0;
        let y = target.clamp(0.0, (content_h - viewport_h).max(0.0));
        self.scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(-y)));
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
            // Воркспейс (7.4): путь последнего открытого дерева.
            workspace: self.tree.as_ref().map(|t| t.root().to_path_buf()),
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
                    ws.refresh_all_languages(cx);
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
            // Горизонтальный скролл не свапится по вкладкам (MVP):
            // сброс — входящая вкладка начинает от левого края.
            self.h_scroll.set_offset(point(px(0.0), px(0.0)));
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
                    // Обёртка таб-бара несёт flex_1/min_w_0 (полоса —
                    // оверлей внутри, tab_bar.rs).
                    .child(self.render_tab_bar(cx))
                    .child(reorder_buttons),
            )
                    .child(self.render_editor(window, cx));

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
            .on_action(cx.listener(Self::on_open_workspace))
            .on_action(cx.listener(Self::on_new_file))
            .on_action(cx.listener(Self::on_new_folder))
            .on_action(cx.listener(Self::on_rename_node))
            .on_action(cx.listener(Self::on_delete_node))
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
            .on_action(cx.listener(Self::on_find_in_file))
            .on_action(cx.listener(Self::on_find_in_tabs))
            .on_action(cx.listener(Self::on_find_in_folder))
            // Дроп мимо папок дерева: сброс подсветки (drag закончился
            // вне drop-целей — on_drag_move больше не придёт).
            .on_drop::<std::path::PathBuf>(cx.listener(|this, _dragged: &std::path::PathBuf, _w, cx| {
                if this.drop_target.take().is_some() {
                    cx.notify();
                }
            }))
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
        // Контекстное меню дерева (с подложкой) и диалог имён — дети
        // корня (паттерн goto/language overlay, D9.2).
        if self.context_menu.is_some() {
            root = root.child(self.render_context_menu(cx));
        }
        if self.tree_dialog.is_some() {
            root = root.child(self.render_tree_dialog(cx));
        }
        let frame_ms = frame_started.elapsed().as_secs_f64() * 1000.0;
        crate::perf::note_frame(frame_ms);
        if crate::bench::enabled() {
            crate::bench::record_render(frame_ms);
        }
        root
    }
}
