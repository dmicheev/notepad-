//! Workspace — корневой view приложения: сборка layout (левая панель
//! «Project» — project_panel.rs, панели split view с таб-барами —
//! tab_bar.rs и редакторами — editor_view.rs, статус-бар —
//! status_bar.rs, overlay Cmd+G — goto_dialog.rs), сессия
//! (persist/session_state).
//!
//! Split view (8.1/D15.1): `panes` (1..=MAX_PANES) — независимые
//! группы вкладок (Pane: модель + view-состояние, pane.rs);
//! активная панель получает действия/поиск, клик активирует.
//!
//! Редактирование — только через tab_model → editor-core
//! (Document::apply + History); курсоры смещаются правками через
//! map_selection. Обработчики действий — editor_actions.rs, мышь —
//! editor_mouse.rs, геометрия — geometry.rs.

use gpui::{Context, FocusHandle, Window, div, point, prelude::*, px, rgb};

use crate::actions::Quit;
use crate::pane::Pane;
use crate::session::{self, SessionState, WindowGeometry};

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
    /// Панели split view (8.1): всегда ≥ 1, ≤ MAX_PANES.
    pub panes: Vec<Pane>,
    /// Индекс активной панели (действия меню, поиск, ввод).
    pub active_pane: usize,
    /// Синхронная прокрутка панелей (8.1): выключена по умолчанию.
    pub sync_scroll: bool,
    /// Выбор темы (8.2): System — по системной настройке.
    pub theme_kind: crate::theme::ThemeKind,
    /// Шрифт редактора (8.3): семейство + размер, применяется
    /// немедленно, сохраняется в сессии.
    pub font: crate::geometry::FontConfig,
    /// Overlay выбора шрифта (8.3): буфер фильтра семейств.
    pub font_dialog: Option<String>,
    pub(crate) font_focus: gpui::FocusHandle,
    /// Кэш системных семейств шрифта (all_font_names, 8.3).
    pub(crate) all_fonts: Vec<String>,
    /// Overlay выбора языка интерфейса (8.4): открыт/закрыт.
    pub ui_lang_dialog: bool,
    pub(crate) ui_lang_focus: gpui::FocusHandle,
    /// Текущий язык интерфейса (8.4; зеркало глобали — для сессии).
    pub ui_language: String,
    /// Оверлей-справочник сочетаний (8.5).
    pub shortcuts_open: bool,
    pub(crate) shortcuts_focus: gpui::FocusHandle,
    pub focus: FocusHandle,
    /// Транзиентное сообщение статус-бара (ошибки открытия/сохранения,
    /// уведомления тумблеров): подменяет левый блок на STATUS_TTL,
    /// затем самоочищается (таймер; поколение защищает новое сообщение
    /// от очистки старым таймером).
    pub status_message: Option<String>,
    /// Поколение сообщения (каждый set инкрементирует).
    pub(crate) status_generation: u64,
    /// Кэш заголовка окна (обновляем в render при изменении).
    pub(crate) rendered_title: Option<String>,
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
    /// собирают до окна, D12.4). (Панель, индекс вкладки).
    pub(crate) pending_threshold_tabs: Vec<(usize, usize)>,
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
    /// Полосы прокрутки (scrollbar.rs): handle левой панели и оверлея
    /// языка (редакторы и таб-бары — в панелях); активный драг
    /// ползунка (move/up слушают контейнеры зон).
    pub(crate) panel_scroll: gpui::ScrollHandle,
    pub(crate) language_scroll: gpui::ScrollHandle,
    pub(crate) thumb_drag: Option<crate::scrollbar::ThumbDrag>,
    /// Предпросмотр Markdown (markdown-preview): тумблер + скролл +
    /// кэш модели; None-семантики нет — панель живёт, пока включена.
    pub(crate) markdown_preview: crate::markdown_preview::MarkdownPreview,
    /// Тулбар (toolbar-and-shortcuts): виден по умолчанию, тумблер
    /// View → Toolbar, состояние — в сессии.
    pub toolbar_visible: bool,
    /// Открытый дропдаун тулбара (одновременно не больше одного).
    pub(crate) toolbar_dropdown: Option<crate::toolbar::DropdownKind>,
    /// Позиция открытия дропдауна (точка клика, anchored).
    pub(crate) toolbar_dropdown_pos: Option<gpui::Point<gpui::Pixels>>,
    /// Наведённая кнопка тулбара (id + позиция для тултипа).
    pub(crate) toolbar_hover: Option<(&'static str, gpui::Point<gpui::Pixels>)>,
    /// Фокус открытого дропдауна (Esc закрывает, спека «Выпадающие
    /// группы»; клавиши не идут в редактор).
    pub(crate) toolbar_dd_focus: gpui::FocusHandle,
    /// Строка поиска окна подсказок (пустая — показать всё).
    pub(crate) shortcuts_query: String,
    /// Настройки воркспейса (.settings/workspace.toml): метки строк
    /// (закладки/фолды) по файлам; пустые — воркспейса нет/файла нет.
    pub(crate) workspace_settings: crate::workspace_settings::WorkspaceSettings,
}

impl Workspace {
    /// Максимум панелей split view (спека 8.1: «2 (и до 4)»).
    pub const MAX_PANES: usize = 4;

    /// Сколько секунд транзиентное сообщение держится в статус-баре,
    /// прежде чем вернуть кодировку/переводы/позицию/язык.
    pub const STATUS_MESSAGE_TTL: u64 = 4;

    pub fn new(cx: &mut Context<Self>) -> Self {
        Workspace {
            panes: vec![Pane::new(cx)],
            active_pane: 0,
            sync_scroll: false,
            theme_kind: crate::theme::ThemeKind::System,
            font: crate::geometry::FontConfig::default(),
            font_dialog: None,
            font_focus: cx.focus_handle(),
            all_fonts: Vec::new(),
            ui_lang_dialog: false,
            ui_lang_focus: cx.focus_handle(),
            ui_language: "en".to_string(),
            shortcuts_open: false,
            shortcuts_focus: cx.focus_handle(),
            focus: cx.focus_handle(),
            status_message: None,
            status_generation: 0,
            rendered_title: None,
            char_advance: None,
            goto_input: None,
            goto_focus: cx.focus_handle(),
            language_input: None,
            language_focus: cx.focus_handle(),
            search: None,
            search_focus: cx.focus_handle(),
            syntax_warmup_scheduled: false,
            pending_threshold_tabs: Vec::new(),
            tree: None,
            tree_dialog: None,
            tree_focus: cx.focus_handle(),
            context_menu: None,
            drop_target: None,
            fs_watch: None,
            watch_generation: 0,
            panel_scroll: gpui::ScrollHandle::new(),
            language_scroll: gpui::ScrollHandle::new(),
            thumb_drag: None,
            markdown_preview: crate::markdown_preview::MarkdownPreview::new(),
            toolbar_visible: true,
            toolbar_dropdown: None,
            toolbar_dropdown_pos: None,
            toolbar_hover: None,
            toolbar_dd_focus: cx.focus_handle(),
            shortcuts_query: String::new(),
            workspace_settings: crate::workspace_settings::WorkspaceSettings::default(),
        }
    }

    /// Прогрев подсветки (5.1/D12.1): инициализация набора синтаксисов
    /// (~сотни мс) + детект языков вкладок + первый разбор активной
    /// (ленивые контексты/regex'ы синтаксиса — вне рендера). Вызывается
    /// отложенно после первого кадра — холодный старт и открытие файлов
    /// syntect не платят. Прогревает ВСЕ панели (split view).
    pub fn refresh_all_languages(&mut self, cx: &mut Context<Self>) {
        for pane in &mut self.panes {
            for tab in pane.model.tabs_mut() {
                tab.refresh_language();
            }
            // Прогреть грамматику активной вкладки панели (ленивая
            // десериализация контекстов + компиляция regex'ов).
            if let Some(tab) = pane.model.active_mut()
                && tab.highlight.syntax_name().is_some() {
                    let lines = tab.doc.len_lines();
                    tab.highlight.ensure_window(&tab.doc, 0, lines.min(64));
            }
        }
        // Фоновый проход чекпоинтами — сразу, не ждать пользовательских
        // прыжков (большой файл красится сам, прыжок вглубь дёшев).
        for pi in 0..self.panes.len() {
            for ix in 0..self.panes[pi].model.len() {
                self.spawn_background_highlight(pi, ix, cx);
            }
        }
    }

    /// Фоновая раскраска файла чекпоинтами (BackgroundWalk) — ходка
    /// ВЕСЬ файл сразу после открытия, не дожидаясь пользовательских
    /// прыжков: после неё любой прыжок = ≤ CHECKPOINT_EVERY строк
    /// кадрового догона. Тактики: syntect/onig-состояния НЕ Send —
    /// фоновой поток невозможен; ходка идёт НА ГЛАВНОМ потоке таймером
    /// (тик 25 мс, бюджет 5 мс — между кадрами, UI не рвётся). Правка
    /// НИЖЕ курсора ходки безвредна (rope актуален, стек границы
    /// валиден); правка ВЫШЕ (bg_valid_upto < line) — ходка
    /// перезапускается от дальнего валидного чекпоинта движка после
    /// паузы (1 с тишины). Смена языка/закрытие вкладки — стоп.
    /// Мелкие файлы (≤ 2 чекпоинтов) не ходим — кадровый
    /// ensure_window дешевле.
    pub(crate) fn spawn_background_highlight(
        &mut self,
        pi: usize,
        ix: usize,
        cx: &mut Context<Self>,
    ) {
        /// Тик ходки (главный поток, между кадрами).
        const TICK: std::time::Duration = std::time::Duration::from_millis(25);
        /// CPU-бюджет порции на тик — больше кадра не занимать.
        const TICK_BUDGET: std::time::Duration = std::time::Duration::from_millis(5);
        /// Пауза перед перезапуском ходки после правок (тишина ввода).
        const RESTART_PAUSE: std::time::Duration = std::time::Duration::from_secs(1);
        let Some(tab) = self.panes.get_mut(pi).and_then(|p| p.model.tab_mut(ix)) else {
            return;
        };
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
                    let Some(tab) = ws
                        .panes
                        .get_mut(pi)
                        .and_then(|p| p.model.tab_mut(ix))
                    else {
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

    /// Показать транзиентное сообщение статус-бара: живёт
    /// [`STATUS_MESSAGE_TTL`], затем левый блок (кодировка/переводы/
    /// позиция/язык) возвращается. Повторный вызов перезапускает
    /// таймер (старый тик молча отбрасывается по поколению).
    pub(crate) fn set_status_message(&mut self, msg: String, cx: &Context<Self>) {
        self.status_message = Some(msg);
        self.status_generation += 1;
        let generation = self.status_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(Self::STATUS_MESSAGE_TTL))
                .await;
            this.update(cx, |ws, cx| {
                if ws.status_generation == generation && ws.status_message.is_some() {
                    ws.status_message = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Транзиентное сообщение при открытии файла сверх порога (5.3):
    /// подсветка отключена, редактирование доступно. Вызывается из
    /// точек открытия (диалог, сессия, NOTEPAD_OPEN).
    #[allow(dead_code)] // уведомление порога живёт в ask_pending_threshold (D12.4);
                        // метод оставлен как API точек открытия (паттерн D10.4)
    pub(crate) fn note_highlight_threshold(&mut self, cx: &gpui::App) {
        if let Some(tab) = self.model().active()
            && tab.language.disabled() {
                self.status_message = Some(crate::i18n::t(cx, "hl.off_threshold"));
            }
    }

    /// Поставить вкладки сверх порога в очередь на вопрос (D12.4):
    /// диалог показывается, когда доступен Window (сессия/NOTEPAD_OPEN
    /// открывают вкладки до окна). Bench-режим не спрашивает.
    pub fn queue_threshold_ask(&mut self) {
        if crate::bench::enabled() {
            return;
        }
        for pi in 0..self.panes.len() {
            for i in 0..self.panes[pi].model.len() {
                let over = self
                    .panes[pi]
                    .model
                    .tab_mut(i)
                    .is_some_and(|t| t.language.over_threshold && !t.language.threshold_allowed);
                if over && !self.pending_threshold_tabs.contains(&(pi, i)) {
                    self.pending_threshold_tabs.push((pi, i));
                }
            }
        }
    }

    /// Спросить пользователя о подсветке файлов сверх порога 20 МБ
    /// (правка спеки окт 2026): согласие — подсветка любого размера
    /// (догон с бюджетом держит кадры), отказ — выключена, сообщение.
    pub fn ask_pending_threshold(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let indices = std::mem::take(&mut self.pending_threshold_tabs);
        let indices: Vec<(usize, usize)> = indices
            .into_iter()
            .filter(|&(pi, i)| {
                self.panes
                    .get_mut(pi)
                    .and_then(|p| p.model.tab_mut(i))
                    .is_some_and(|t| t.language.over_threshold && !t.language.threshold_allowed)
            })
            .collect();
        if indices.is_empty() || crate::bench::enabled() {
            return;
        }
        let count = indices.len();
        let single = count == 1;
        let message = if single {
            crate::i18n::t(cx, "dialog.threshold_single")
        } else {
            crate::i18n::tf(cx, "dialog.threshold_many", &[("n", &count.to_string())])
        };
        let rx = window.prompt(
            gpui::PromptLevel::Warning,
            &message,
            Some(&crate::i18n::t(cx, "dialog.threshold_note")),
            &[
                crate::i18n::t(cx, "dialog.enable").as_str(),
                crate::i18n::t(cx, "dialog.keep_off").as_str(),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = rx.await else { return };
            this.update_in(cx, |ws, _window, cx| {
                match choice {
                    0 => {
                        for (pi, i) in indices {
                            if let Some(tab) =
                                ws.panes.get_mut(pi).and_then(|p| p.model.tab_mut(i))
                            {
                                tab.allow_highlight_despite_threshold();
                                // Язык включён — фоновая ходка чекпоинтами
                                // (пропущенная на открытии: синтаксиса не было).
                                ws.spawn_background_highlight(pi, i, cx);
                            }
                        }
                        ws.status_message =
                            Some(crate::i18n::t(cx, "hl.enabled_large"));
                    }
                    _ => {
                        ws.status_message =
                            Some(crate::i18n::t(cx, "hl.off_threshold"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Применить операцию к активной вкладке активной панели (вкладок
    /// нет — no-op) и перерисовать. Схлопывает повтор
    /// `if let Some(tab) = ...; cx.notify()` обработчиков действий
    /// (editor_actions.rs).
    pub(crate) fn with_active_tab(
        &mut self,
        op: impl FnOnce(&mut crate::buffer_tab::BufferTab),
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.model_mut().active_mut() {
            op(tab);
        }
        cx.notify();
    }

    /// Пересчитать мапу видимых строк панели `pi`, если устарела.
    /// Вызывается перед любым использованием мапы (рендер, координаты
    /// мыши, скролл). Активная панель — `ensure_fold_view`.
    ///
    /// Правила инвалидации (D11.2): тождественная мапа живёт, пока
    /// фолдов нет (правки текста folded-флаги не трогают); материальная
    /// мапа пересобирается при ЛЮБОМ изменении view_version и при
    /// уходе фолдов в ноль (тогда — обратно в тождественную, бесплатно).
    /// Undo/redo меняет folded-флаги только вместе с LineMeta-синком
    /// правок (Document::apply) — счётчик folded_count это покрывает:
    /// 0→N ловится переходом Identity→Mapped, N→0 — обратно.
    pub(crate) fn ensure_fold_view_of(&mut self, pi: usize) {
        let Some(ix) = self.panes[pi].model.active_index() else {
            self.panes[pi].fold_view = None;
            return;
        };
        let folded = self.panes[pi].model.tabs()[ix]
            .doc
            .line_meta()
            .folded_count();
        let version = self.panes[pi].model.tabs()[ix].view_version();
        let need = match &self.panes[pi].fold_view {
            Some((cached_ix, view)) if *cached_ix == ix => match view {
                FoldView::Identity => folded > 0,
                FoldView::Mapped { version: v, .. } => folded == 0 || *v != version,
            },
            _ => true, // вкладки сменились / кэша нет
        };
        if need {
            let doc = &self.panes[pi].model.tabs()[ix].doc;
            self.panes[pi].fold_view = Some((
                ix,
                if folded == 0 {
                    FoldView::Identity
                } else {
                    FoldView::Mapped { version, map: crate::folding::visible_lines(doc) }
                },
            ));
        }
    }

    /// Пересчёт мапы активной панели.
    pub(crate) fn ensure_fold_view(&mut self) {
        self.ensure_fold_view_of(self.active_pane);
    }

    /// Индекс uniform_list (видимый) → real-строка документа (мапа
    /// folding, D9.5) панели `pi`.
    pub(crate) fn visible_to_real_of(&self, pi: usize, ix: usize) -> usize {
        match &self.panes[pi].fold_view {
            Some((_, FoldView::Mapped { map, .. })) => map.get(ix).copied().unwrap_or(ix),
            _ => ix,
        }
    }

    /// Сколько элементов сейчас в uniform_list (видимых строк) панели.
    pub(crate) fn visible_len_of(&self, pi: usize) -> usize {
        match &self.panes[pi].fold_view {
            Some((_, FoldView::Identity)) => self.panes[pi]
                .model
                .active()
                .map(|t| t.doc.len_lines())
                .unwrap_or(0),
            Some((_, FoldView::Mapped { map, .. })) => map.len(),
            None => 0,
        }
    }

    /// Активная панель: видимый индекс → real-строка.
    pub(crate) fn visible_to_real(&self, ix: usize) -> usize {
        self.visible_to_real_of(self.active_pane, ix)
    }

    /// Активная панель: число видимых строк.
    pub(crate) fn visible_len(&self) -> usize {
        self.visible_len_of(self.active_pane)
    }

    /// Прокрутить активную панель к real-строке (через мапу видимых),
    /// стратегия — центр вьюпорта. Скрытая строка сначала поднимается
    /// к заголовку фолда (D9.5).
    pub(crate) fn scroll_to_line(&mut self, line: usize) {
        self.ensure_fold_view();
        let line = self
            .model()
            .active()
            .map(|t| t.visible_line_or_header(line))
            .unwrap_or(line);
        let vis = self.real_to_visible(line);
        self.pane_mut()
            .scroll
            .scroll_to_item(vis, gpui::ScrollStrategy::Center);
    }

    /// Немедленная прокрутка активной панели к real-строке с
    /// центрированием (переходы поиска 6.1–6.4): ловушка
    /// deferred-скролла D12.5 — `scroll_to_item` применяется в
    /// prepaint ПОСЛЕ render, кадр после клика по ‹/› строится по
    /// СТАРОМУ offset и в живом окне «догонял» только следующим
    /// событием. Ставим целевой offset в общий Rc (тот же, что читает
    /// render) НЕМЕДЛЕННО + `scroll_to_item` тем же индексом —
    /// deferred подтвердит значение (паттерн клика по липкой шапке
    /// D12.5).
    pub(crate) fn scroll_to_line_now(&mut self, line: usize) {
        self.ensure_fold_view();
        let line = self
            .model()
            .active()
            .map(|t| t.visible_line_or_header(line))
            .unwrap_or(line);
        let vis = self.real_to_visible(line);
        let viewport_h = self
            .editor_bounds()
            .map(|b| f32::from(b.size.height))
            .unwrap_or(0.0);
        let scroll = self.pane().scroll.clone();
        if viewport_h <= 0.0 {
            // Редактор ещё не отрисован — только deferred-путь.
            scroll.scroll_to_item(vis, gpui::ScrollStrategy::Center);
            return;
        }
        let line_h = self.line_height();
        let content_h = self.visible_len() as f32 * line_h;
        // Центр вьюпорта на середине строки (семантика Center).
        let target = vis as f32 * line_h + line_h / 2.0 - viewport_h / 2.0;
        let y = target.clamp(0.0, (content_h - viewport_h).max(0.0));
        scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(-y)));
        scroll.scroll_to_item(vis, gpui::ScrollStrategy::Center);
    }

    /// real-строка → индекс uniform_list (видимый) активной панели;
    /// скрытая строка — её заголовок. Тождественная мапа: clamp в
    /// последнюю строку (как fallback материальной).
    fn real_to_visible(&self, line: usize) -> usize {
        let line = self
            .model()
            .active()
            .map(|t| t.visible_line_or_header(line))
            .unwrap_or(line);
        match &self.pane().fold_view {
            Some((_, FoldView::Mapped { map, .. })) => {
                crate::folding::real_to_visible(map, line)
                    .unwrap_or_else(|| map.len().saturating_sub(1))
            }
            _ => line.min(self.visible_len().saturating_sub(1)),
        }
    }

    /// Свап scroll-позиции вкладок панели `pi` (D8.4): уходящая вкладка
    /// запоминает текущее смещение, входящая восстанавливает; соседние
    /// панели не трогаются. Горизонтальный скролл не свапится (MVP):
    /// сброс — входящая вкладка начинает от левого края.
    pub(crate) fn swap_pane_scroll(&mut self, pi: usize) {
        let active_ix = self.panes[pi].model.active_index();
        if self.panes[pi].scroll_owner != active_ix {
            if let Some(owner) = self.panes[pi].scroll_owner {
                let y = f32::from(
                    self.panes[pi]
                        .scroll
                        .0
                        .borrow()
                        .base_handle
                        .offset()
                        .y,
                );
                if let Some(tab) = self.panes[pi].model.tab_mut(owner) {
                    tab.scroll_y = y;
                }
            }
            if let Some(ix) = active_ix {
                let y = self.panes[pi].model.tabs()[ix].scroll_y;
                self.panes[pi]
                    .scroll
                    .0
                    .borrow()
                    .base_handle
                    .set_offset(point(px(0.0), px(y)));
            }
            self.panes[pi].h_scroll.set_offset(point(px(0.0), px(0.0)));
            self.panes[pi].scroll_owner = active_ix;
        }
    }

    // Геометрия (editor_bounds / char_advance / visible_line_at_y /
    // col_at_x / page_lines) — в geometry.rs, единый источник (B).

    // Мышь в редакторе (4.2/4.3): DragState, pos_to_line_col и
    // обработчики down/move/up — в editor_mouse.rs.

    // ---- Сессия (3.4) ----

    /// Собрать состояние сессии из моделей ВСЕХ паней: только вкладки
    /// с путями (untitled не восстанавливаются — D8.4), активная —
    /// пересчитанный индекс в объединённом списке. Раскладка split не
    /// восстанавливается (MVP 8.1): все вкладки открываются в одну
    /// панель.
    pub fn session_state(&self, geometry: Option<WindowGeometry>) -> SessionState {
        let mut tabs = Vec::new();
        let mut active_tab = None;
        for (pi, pane) in self.panes.iter().enumerate() {
            for (i, t) in pane.model.tabs().iter().enumerate() {
                let Some(path) = &t.path else { continue };
                if self.active_pane == pi && pane.model.active_index() == Some(i) {
                    active_tab = Some(tabs.len());
                }
                tabs.push(session::SessionTab { path: path.clone() });
            }
        }
        SessionState {
            // Воркспейс (7.4): путь последнего открытого дерева.
            workspace: self.tree.as_ref().map(|t| t.root().to_path_buf()),
            tabs,
            active_tab,
            // Тема (8.2): выбор сохраняется между сессиями.
            theme: Some(self.theme_kind.as_str().to_string()),
            // Шрифт (8.3): настройка сохраняется между сессиями.
            font: Some(session::FontSettings {
                family: self.font.family.clone(),
                size: self.font.size,
            }),
            // Язык интерфейса (8.4).
            language: Some(self.ui_language.clone()),
            // Тулбар (toolbar-and-shortcuts): видимость.
            toolbar_visible: Some(self.toolbar_visible),
            window: geometry,
        }
    }

    /// Сохранить сессию (синхронно; файл маленький) с текущей геометрией
    /// окна. Вызывается из on_window_should_close и on_app_quit; перед
    /// сессией — метки строк воркспейса (.settings).
    pub fn persist(&mut self, window: &Window) {
        self.persist_workspace_marks();
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

impl Workspace {
    /// Область панелей редактора (8.1/D15.1): 1 — целиком; 2 — строка
    /// с вертикальным разделителем; 3–4 — сетка 2×2 (3-я панель
    /// занимает нижнюю строку). Каждая панель — своя колонка
    /// [таб-бар | редактор]; таб-бары независимы (вкладки панелей).
    fn render_pane_area(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // Разделители 1px между панелями (читаемость границ).
        fn v_divider() -> gpui::Div {
            div()
                .w(px(1.0))
                .flex_shrink_0()
                .bg(rgb(crate::theme::current().tabbar))
        }
        fn h_divider() -> gpui::Div {
            div()
                .h(px(1.0))
                .flex_shrink_0()
                .bg(rgb(crate::theme::current().tabbar))
        }
        match self.panes.len() {
            1 => self.render_pane(0, window, cx).into_any_element(),
            2 => div()
                .id("pane-row")
                .flex()
                .flex_1()
                .min_h_0()
                .child(self.render_pane(0, window, cx))
                .child(v_divider())
                .child(self.render_pane(1, window, cx))
                .into_any_element(),
            n => {
                // 3–4: сетка 2×2 — строки по две панели.
                let mut bottom = div()
                    .id("pane-row-bottom")
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_pane(2, window, cx));
                if n == 4 {
                    bottom = bottom
                        .child(v_divider())
                        .child(self.render_pane(3, window, cx));
                }
                div()
                    .id("pane-grid")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("pane-row-top")
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .child(self.render_pane(0, window, cx))
                            .child(v_divider())
                            .child(self.render_pane(1, window, cx)),
                    )
                    .child(h_divider())
                    .child(bottom)
                    .into_any_element()
            }
        }
    }

    /// Одна панель: колонка [таб-бар (+кнопки) | редактор]. Клик по
    /// панели активирует её (фокус редактора — паттерн VS Code).
    fn render_pane(
        &mut self,
        pi: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .id(("pane", pi))
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _e, window, cx| {
                    this.focus_pane(pi, window, cx);
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .bg(rgb(crate::theme::current().tabbar))
                    // Обёртка таб-бара несёт flex_1/min_w_0 (полоса —
                    // оверлей внутри, tab_bar.rs).
                    .child(self.render_tab_bar(pi, cx))
                    .child(self.render_reorder_buttons(pi, cx)),
            )
            .child(self.render_editor(pi, window, cx))
            .into_any_element()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Бенчмарк-стенд (D11.1) и панель PERFORMANCE (D11.8): один
        // Instant-замер — CPU-стоимость render (ensure_fold_view +
        // элементное дерево), без vsync-ожидания.
        let frame_started = std::time::Instant::now();

        // Тема (8.2): разрешение выбора на каждом кадре — System
        // следует системной настройке без перезапуска; смена палитры
        // перерисовывает всё (рендеры читают theme::current()).
        let resolved = self.theme_kind.resolve(window.appearance());
        if !std::ptr::eq(resolved, crate::theme::current()) {
            crate::theme::set_current(resolved);
        }

        // Мапы видимых строк (folding, D9.5) — пересчёт при устаревании,
        // для КАЖДОЙ панели. До них — гидратация меток из настроек
        // воркспейса (.settings): закладки/фолды применяются до сборки
        // фолд-мапы первого кадра.
        self.hydrate_workspace_marks();
        for pi in 0..self.panes.len() {
            self.ensure_fold_view_of(pi);
        }

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

        // Заголовок окна = активная вкладка активной панели (кэш — set
        // только при изменении, чтобы не перерисовывать окно каждый кадр).
        let desired_title = self
            .model()
            .active_index()
            .and_then(|i| self.model().title(i))
            .unwrap_or_else(|| "notepad+".to_string());
        if self.rendered_title.as_deref() != Some(desired_title.as_str()) {
            window.set_window_title(&desired_title);
            self.rendered_title = Some(desired_title);
        }

        // Свап scroll-позиций при переключении вкладок (D8.4) — для
        // каждой панели независимо.
        for pi in 0..self.panes.len() {
            self.swap_pane_scroll(pi);
        }

        // Левая панель — свой модуль.
        let project_panel = self.render_project_panel(cx);

        // Тулбар (toolbar-and-shortcuts): полоса над всем контентом,
        // включается тумблером View → Toolbar (по умолчанию виден).
        let toolbar = self
            .toolbar_visible
            .then(|| self.render_toolbar(cx));

        // Центральная колонка: [область панелей | разделитель |
        // предпросмотр Markdown] (markdown-preview: ~40% ширины,
        // минимум 320 px; разделитель ДРАГАЕТ ширины — двойной клик
        // сброс; раскладка split-панелей не перестраивается).
        let mut center = div().flex().flex_1().min_w_0().child(self.render_pane_area(window, cx));
        if self.markdown_preview.open {
            center = center
                .child(self.render_preview_divider(cx))
                .child(self.render_markdown_preview(cx));
        }

        // Корень: [ панель | центр ] по горизонтали + статус-бар внизу.
        // Действия вешаем на корень (а не на редактор): работают и при
        // пустом списке вкладок (фокус в редакторе-заглушке).
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(crate::theme::current().bg))
            .text_color(rgb(crate::theme::current().text))
            .on_action(cx.listener(Self::on_open))
            .on_action(cx.listener(Self::on_open_workspace))
            .on_action(cx.listener(Self::on_new_file))
            .on_action(cx.listener(Self::on_new_folder))
            .on_action(cx.listener(Self::on_rename_node))
            .on_action(cx.listener(Self::on_delete_node))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_save_as))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_split_editor))
            .on_action(cx.listener(Self::on_close_pane))
            .on_action(cx.listener(Self::on_toggle_sync_scroll))
            .on_action(cx.listener(Self::on_theme_system))
            .on_action(cx.listener(Self::on_theme_dark))
            .on_action(cx.listener(Self::on_theme_light))
            .on_action(cx.listener(Self::on_open_font_dialog))
            .on_action(cx.listener(Self::on_zoom_in))
            .on_action(cx.listener(Self::on_zoom_out))
            .on_action(cx.listener(Self::on_zoom_reset))
            .on_action(cx.listener(Self::on_open_ui_language_dialog))
            .on_action(cx.listener(Self::on_new_tab))
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_prev_tab))
            .on_action(cx.listener(Self::on_show_shortcuts))
            .on_action(cx.listener(Self::on_cycle_language))
            .on_action(cx.listener(Self::on_toggle_markdown_preview))
            .on_action(cx.listener(Self::on_toggle_toolbar))
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
            }));
        // Тулбар — первой полосой корня (над контентом, под оверлеями).
        if let Some(toolbar) = toolbar {
            root = root.child(toolbar);
        }
        root = root
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
        if self.font_dialog.is_some() {
            root = root.child(self.render_font_overlay(cx));
        }
        if self.ui_lang_dialog {
            root = root.child(self.render_ui_language_overlay(cx));
        }
        if self.shortcuts_open {
            root = root.child(self.render_shortcuts_overlay(cx));
        }
        if self.toolbar_dropdown.is_some() {
            root = root.child(self.render_toolbar_dropdown(cx));
        }
        // Захват мыши драгом разделителя предпросмотра — ПОСЛЕДНИМ
        // ребёнком: топ-слой, move/up драга приходят поверх всех.
        if self.markdown_preview.drag {
            root = root.child(self.render_preview_drag_overlay(cx));
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
