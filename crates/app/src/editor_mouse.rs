//! Мышь в редакторе (4.2/4.3, D9.2/D9.3 + 8.1): перевод координат
//! окна в (строка, колонка) и стейт-машина драга — обычное выделение,
//! column selection (Alt/Cmd+drag), Cmd+клик (добавить/снять курсор),
//! Shift+клик (расширение), двойной/тройной клик (слово/строка).
//!
//! Split view: слушатели регистрируются редактором КАЖДОЙ панели с её
//! индексом; down активирует панель, драг/скролл ведутся её моделью и
//! scroll-handle. Драг, ушедший курсором в соседнюю панель, продолжает
//! считаться геометрию по своей панели (MVP: bounds активной).
//!
//! Ловушка gpui 0.2.2: on_mouse_up слушает КОНТЕЙНЕР редактора —
//! click не срабатывает при драге между элементами, up обязан
//! прилетать и вне строк.

use gpui::{Context, Window, point};

use crate::workspace::Workspace;

/// Состояние мыши в редакторе (4.2): якорь и голова в (строка, колонка).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DragState {
    /// Column selection (Alt/Cmd+drag — 4.3).
    pub(crate) column: bool,
    /// Начали с Cmd (Cmd+drag → колонка; Cmd+клик без движения → курсор).
    pub(crate) cmd: bool,
    pub(crate) anchor: (usize, usize),
    pub(crate) head: (usize, usize),
}

impl DragState {
    pub(crate) fn moved(&self) -> bool {
        self.anchor != self.head
    }
}

impl Workspace {
    /// Диагностика мыши (NOTEPAD_DEBUG_MOUSE=1): каждое событие
    /// логируется в stderr с сырыми координатами и вычисленной
    /// позицией — для разбора «живых» проблем вне тестового стенда.
    pub(crate) fn mouse_debug() -> bool {
        static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *DEBUG.get_or_init(|| std::env::var("NOTEPAD_DEBUG_MOUSE").is_ok())
    }

    /// Wheel редактора панели `pi` с разделением осей (ловушка gpui
    /// 0.2.2): скролл-контейнер с ОДНОЙ осью конвертирует колесо другой
    /// оси в свою (зона с overflow-x превратила бы вертикальное колесо
    /// в горизонтальный скролл; список — горизонтальное в вертикальный).
    /// Здесь дельта раскладывается по осям вручную (формулы внутреннего
    /// слушателя gpui: offset += delta; клампы делает prepaint) и
    /// событие гасится. Вызывают: guard-оверлей зоны (срабатывает
    /// раньше внутренних слушателей — см. комментарий в editor_view)
    /// и дорожки полос (вне зоны — guard их не покрывает). При
    /// включённой синхронной прокрутке — рассинхрон панелей (8.1).
    pub(crate) fn on_editor_wheel_pane(
        &mut self,
        pi: usize,
        event: &gpui::ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let d = event.delta.pixel_delta(window.line_height());
        if Self::mouse_debug() {
            eprintln!("[wheel] dx={:.1} dy={:.1}", f32::from(d.x), f32::from(d.y));
        }
        // Вертикаль — в uniform_list (общий Rc offset с interactivity),
        // горизонталь — в зону; поперечные компоненты сохраняются.
        let v = self.panes[pi].scroll.0.borrow().base_handle.clone();
        let vo = v.offset();
        v.set_offset(point(vo.x, vo.y + d.y));
        let ho = self.panes[pi].h_scroll.offset();
        self.panes[pi].h_scroll.set_offset(point(ho.x + d.x, ho.y));
        if self.sync_scroll {
            self.sync_scroll_from(pi, cx);
        }
        cx.notify();
        cx.stop_propagation();
    }

    /// (real-строка, колонка) из позиции мыши в окне (активная панель).
    pub(crate) fn pos_to_line_col(
        &mut self,
        window: &Window,
        pos: gpui::Point<gpui::Pixels>,
    ) -> Option<(usize, usize)> {
        self.ensure_fold_view();
        let vis = self.visible_line_at_y(f32::from(pos.y))?;
        let real = self.visible_to_real(vis);
        let col = self.col_at_x(window, f32::from(pos.x));
        if Self::mouse_debug() {
            let b = self.editor_bounds();
            let offset = f32::from(self.pane().scroll.0.borrow().base_handle.offset().y);
            let adv = self.char_advance(window);
            let (bo, bs) = b
                .map(|b| {
                    (
                        (f32::from(b.origin.x), f32::from(b.origin.y)),
                        (f32::from(b.size.width), f32::from(b.size.height)),
                    )
                })
                .unwrap_or(((0.0, 0.0), (0.0, 0.0)));
            eprintln!(
                "[mouse] map pos=({:.1},{:.1}) bounds_origin=({:.1},{:.1}) size=({:.1},{:.1}) scroll_y={:.1} advance={:.2} vis={} real={}",
                f32::from(pos.x), f32::from(pos.y), bo.0, bo.1, bs.0, bs.1, offset, adv, vis, real
            );
        }
        Some((real, col))
    }

    pub(crate) fn on_editor_mouse_down_pane(
        &mut self,
        pi: usize,
        event: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Клик в редактор панели — активировать её (до геометрии:
        // pos_to_line_col считает по активной панели).
        self.activate_pane(pi, cx);
        window.focus(&self.panes[pi].focus.clone());
        let Some((line, col)) = self.pos_to_line_col(window, event.position) else {
            return;
        };
        if Self::mouse_debug() {
            let m = &event.modifiers;
            eprintln!(
                "[mouse] down count={} shift={} ctrl={} alt={} cmd={} pos=({:.1},{:.1}) -> (line {}, col {})",
                event.click_count, m.shift, m.control, m.alt, m.platform,
                f32::from(event.position.x), f32::from(event.position.y), line, col
            );
        }
        let Some(tab) = self.panes.get_mut(pi).and_then(|p| p.model.active_mut()) else { return };
        let m = &event.modifiers;
        if m.shift {
            // Shift+клик — расширить выделение до позиции.
            tab.select_to(line, col);
            cx.notify();
            return;
        }
        if m.alt || m.platform {
            // Alt/Cmd+drag → column selection; Cmd+клик без движения
            // на mouse_up добавит курсор (D9.3).
            self.panes[pi].drag = Some(DragState {
                column: true,
                cmd: m.platform,
                anchor: (line, col),
                head: (line, col),
            });
        } else {
            // Обычные клики (жесты мыши, D9.3): одиночный — каретка,
            // двойной — слово, тройной — строка.
            match event.click_count {
                2 => tab.select_word_at(line, col),
                n if n >= 3 => tab.select_line_at(line),
                _ => {
                    tab.set_caret_at(line, col);
                    self.panes[pi].drag = Some(DragState {
                        column: false,
                        cmd: false,
                        anchor: (line, col),
                        head: (line, col),
                    });
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn on_editor_mouse_move_pane(
        &mut self,
        pi: usize,
        event: &gpui::MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Драг ползунка полосы прокрутки — раньше выделения (полоса —
        // сосед uniform_list, её move/up приходят сюда же).
        if self.on_scrollbar_drag_move(event, window, cx) {
            return;
        }
        let Some(mut drag) = self.panes.get(pi).and_then(|p| p.drag) else { return };
        let Some((line, col)) = self.pos_to_line_col(window, event.position) else {
            return;
        };
        if drag.head == (line, col) {
            return;
        }
        if Self::mouse_debug() {
            eprintln!(
                "[mouse] move pos=({:.1},{:.1}) -> (line {}, col {})",
                f32::from(event.position.x), f32::from(event.position.y), line, col
            );
        }
        drag.head = (line, col);
        if let Some(pane) = self.panes.get_mut(pi) {
            pane.drag = Some(drag);
            let Some(tab) = pane.model.active_mut() else { return };
            if drag.column {
                // Живой предпросмотр прямоугольника.
                tab.set_column_block(drag.anchor, drag.head);
            } else {
                // Обычный драг: живое выделение от якоря.
                tab.select_to(line, col);
            }
        }
        cx.notify();
    }

    pub(crate) fn on_editor_mouse_up_pane(
        &mut self,
        pi: usize,
        event: &gpui::MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Завершение драга ползунка — раньше выделения.
        if self.on_scrollbar_drag_up(event, window, cx) {
            return;
        }
        let Some(mut drag) = self
            .panes
            .get_mut(pi)
            .and_then(|p| p.drag.take())
        else { return };
        // Голову драга уточняем фактической позицией отпускания, если
        // move-события её догнали (иначе — последняя известная).
        if let Some(head) = self.pos_to_line_col(window, event.position) {
            drag.head = head;
        }
        if Self::mouse_debug() {
            eprintln!(
                "[mouse] up pos=({:.1},{:.1}) anchor=({},{}) head=({},{}) column={}",
                f32::from(event.position.x), f32::from(event.position.y),
                drag.anchor.0, drag.anchor.1, drag.head.0, drag.head.1, drag.column
            );
        }
        if let Some(tab) = self.panes.get_mut(pi).and_then(|p| p.model.active_mut()) {
            if drag.column {
                if drag.moved() {
                    // Alt/Cmd+drag: блок уже стоит (живой предпросмотр).
                } else if drag.cmd {
                    // Cmd+клик без движения — добавить курсор.
                    tab.add_cursor_at(drag.anchor.0, drag.anchor.1);
                } else {
                    // Alt+клик без движения — просто каретка в якоре.
                    tab.set_caret_at(drag.anchor.0, drag.anchor.1);
                }
            } else if drag.moved() {
                tab.select_to(drag.head.0, drag.head.1);
            }
            // Обычный клик: каретка уже стоит с mouse_down.
        }
        cx.notify();
    }
}
