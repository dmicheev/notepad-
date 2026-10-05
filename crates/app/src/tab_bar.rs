//! Таб-бар (3.2/D8.2): вкладки с индикатором dirty, кнопка «+»,
//! кнопки перестановки ◂ ▸ (drag-and-drop в gpui 0.2.2 требует
//! ghost-Render view — отвергнуто для MVP).

use gpui::{Context, div, prelude::*, px, rgb};

use crate::geometry::TABBAR_H;
use crate::theme::{ACCENT, BG, CLOSE_HOVER, DIRTY_DOT, TAB_INACTIVE, TABBAR, TEXT};
use crate::workspace::Workspace;

impl Workspace {
    pub(crate) fn render_tab_bar(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        // Сам бар скроллится горизонтально (track_scroll даёт handle
        // полосе); полоса — absolute-оверлей низа обёртки, move/up
        // обёртки ведут драг ползунка (scrollbar.rs).
        let mut bar = div()
            .id("tab-bar")
            .flex()
            .items_center()
            .h(px(TABBAR_H))
            .flex_shrink_0()
            .bg(rgb(TABBAR))
            .overflow_x_scroll()
            .track_scroll(&self.tab_scroll);

        for ix in 0..self.model.len() {
            let is_active = self.model.active_index() == Some(ix);
            let title = self.model.title(ix).unwrap_or_default();
            let dirty = self.model.tabs()[ix].dirty;
            let tab = div()
                .id(("tab", ix))
                .on_click(
                    cx.listener(move |this, _, _window, cx| {
                        this.model.activate(ix);
                        cx.notify();
                    }),
                )
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .h_full()
                .flex_shrink_0()
                .text_size(px(12.0))
                .text_color(rgb(if is_active { TEXT } else { TAB_INACTIVE }))
                .when(is_active, |d| {
                    d.bg(rgb(BG)).border_t_2().border_color(rgb(ACCENT))
                })
                .when(dirty, |d| d.child(div().text_color(rgb(DIRTY_DOT)).child("●")))
                .child(title)
                .child(
                    div()
                        .id(("tab-close", ix))
                        .on_click(cx.listener(move |this, _ev, window, cx| {
                            cx.stop_propagation();
                            this.close_tab_at(ix, window, cx);
                        }))
                        .px_1()
                        .hover(|d| d.bg(rgb(CLOSE_HOVER)).rounded_sm())
                        .text_color(rgb(TAB_INACTIVE))
                        .child("×"),
                );
            bar = bar.child(tab);
        }

        let bar = bar.child(
            div()
                .id("new-tab")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.model.new_untitled();
                    cx.notify();
                }))
                .px_2()
                .h_full()
                .flex()
                .items_center()
                .text_color(rgb(TAB_INACTIVE))
                .hover(|d| d.bg(rgb(CLOSE_HOVER)))
                .child("+"),
        );

        // Обёртка: bar (фиксированная высота) + полоса-оверлей.
        div()
            .id("tab-bar-zone")
            .relative()
            .flex_1()
            .min_w_0()
            .on_mouse_move(cx.listener(
                |this, event: &gpui::MouseMoveEvent, window, cx| {
                    this.on_scrollbar_drag_move(event, window, cx);
                },
            ))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseUpEvent, window, cx| {
                    this.on_scrollbar_drag_up(event, window, cx);
                }),
            )
            .child(bar)
            .child(
                self.render_horizontal_scrollbar(crate::scrollbar::ScrollTarget::TabBar, cx),
            )
    }

    /// Кнопки перестановки вкладок (кнопки, не drag — D8.2).
    pub(crate) fn render_reorder_buttons(&self, cx: &mut Context<Self>) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .flex_shrink_0()
        .bg(rgb(TABBAR))
        .child(
            div()
                .id("move-left")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.model.move_active(-1);
                    cx.notify();
                }))
                .px_2()
                .h(px(TABBAR_H))
                .flex()
                .items_center()
                .text_size(px(11.0))
                .text_color(rgb(TAB_INACTIVE))
                .hover(|d| d.bg(rgb(CLOSE_HOVER)))
                .child("◂"),
        )
        .child(
            div()
                .id("move-right")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.model.move_active(1);
                    cx.notify();
                }))
                .px_2()
                .h(px(TABBAR_H))
                .flex()
                .items_center()
                .text_size(px(11.0))
                .text_color(rgb(TAB_INACTIVE))
                .hover(|d| d.bg(rgb(CLOSE_HOVER)))
                .child("▸"),
        )
    }
}
