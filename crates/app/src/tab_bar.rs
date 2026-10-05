//! Таб-бар (3.2/D8.2 + 8.1/D15.1): вкладки с индикатором dirty, кнопка
//! «+», кнопки перестановки ◂ ▸ (drag-and-drop в gpui 0.2.2 требует
//! ghost-Render view — отвергнуто для MVP). С split view каждый бар
//! принадлежит СВОЕЙ панели: активирует её вкладки; клик по вкладке
//! активирует панель (независимые наборы вкладок).

use gpui::{Context, div, prelude::*, px, rgb};

use crate::geometry::TABBAR_H;
use crate::workspace::Workspace;

impl Workspace {
    pub(crate) fn render_tab_bar(
        &mut self,
        pi: usize,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        // Сам бар скроллится горизонтально (track_scroll даёт handle
        // полосе); полоса — absolute-оверлей низа обёртки, move/up
        // обёртки ведут драг ползунка (scrollbar.rs).
        let mut bar = div()
            .id(("tab-bar", pi))
            .flex()
            .items_center()
            .h(px(TABBAR_H))
            .flex_shrink_0()
            .bg(rgb(crate::theme::current().tabbar))
            .overflow_x_scroll()
            .track_scroll(&self.panes[pi].tab_scroll);

        let model_len = self.panes[pi].model.len();
        for ix in 0..model_len {
            let is_active = self.panes[pi].model.active_index() == Some(ix);
            let title = self.panes[pi].model.title(ix).unwrap_or_default();
            let dirty = self.panes[pi].model.tabs()[ix].dirty;
            let tab = div()
                .id(("tab", pi * 1_000_000_000 + ix))
                .on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.activate_pane(pi, cx);
                        if let Some(pane) = this.panes.get_mut(pi) {
                            pane.model.activate(ix);
                        }
                        let f = this.panes[pi].focus.clone();
                        window.focus(&f);
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
                .text_color(rgb(if is_active { crate::theme::current().text } else { crate::theme::current().tab_inactive }))
                .when(is_active, |d| {
                    d.bg(rgb(crate::theme::current().bg)).border_t_2().border_color(rgb(crate::theme::current().accent))
                })
                .when(dirty, |d| d.child(div().text_color(rgb(crate::theme::current().dirty_dot)).child("●")))
                .child(title)
                .child(
                    div()
                        .id(("tab-close", pi * 1_000_000_000 + ix))
                        .on_click(cx.listener(move |this, _ev, window, cx| {
                            cx.stop_propagation();
                            this.activate_pane(pi, cx);
                            this.close_tab_in_pane(pi, ix, window, cx);
                        }))
                        .px_1()
                        .hover(|d| d.bg(rgb(crate::theme::current().close_hover)).rounded_sm())
                        .text_color(rgb(crate::theme::current().tab_inactive))
                        .child("×"),
                );
            bar = bar.child(tab);
        }

        let bar = bar.child(
            div()
                .id(("new-tab", pi))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.activate_pane(pi, cx);
                    if let Some(pane) = this.panes.get_mut(pi) {
                        pane.model.new_untitled();
                    }
                    let f = this.panes.get(pi).map(|p| p.focus.clone());
                    if let Some(f) = f {
                        window.focus(&f);
                    }
                    cx.notify();
                }))
                .px_2()
                .h_full()
                .flex()
                .items_center()
                .text_color(rgb(crate::theme::current().tab_inactive))
                .hover(|d| d.bg(rgb(crate::theme::current().close_hover)))
                .child("+"),
        );

        // Обёртка: bar (фиксированная высота) + полоса-оверлей.
        div()
            .id(("tab-bar-zone", pi))
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
            .child(self.render_horizontal_scrollbar(
                crate::scrollbar::ScrollTarget::TabBar { pane: pi },
                cx,
            ))
    }

    /// Кнопки перестановки вкладок (кнопки, не drag — D8.2) панели pi.
    pub(crate) fn render_reorder_buttons(
        &mut self,
        pi: usize,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .flex_shrink_0()
            .bg(rgb(crate::theme::current().tabbar))
            .child(
                div()
                    .id(("move-left", pi))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_pane(pi, cx);
                        if let Some(pane) = this.panes.get_mut(pi) {
                            pane.model.move_active(-1);
                        }
                        cx.notify();
                    }))
                    .px_2()
                    .h(px(TABBAR_H))
                    .flex()
                    .items_center()
                    .text_size(px(11.0))
                    .text_color(rgb(crate::theme::current().tab_inactive))
                    .hover(|d| d.bg(rgb(crate::theme::current().close_hover)))
                    .child("◂"),
            )
            .child(
                div()
                    .id(("move-right", pi))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_pane(pi, cx);
                        if let Some(pane) = this.panes.get_mut(pi) {
                            pane.model.move_active(1);
                        }
                        cx.notify();
                    }))
                    .px_2()
                    .h(px(TABBAR_H))
                    .flex()
                    .items_center()
                    .text_size(px(11.0))
                    .text_color(rgb(crate::theme::current().tab_inactive))
                    .hover(|d| d.bg(rgb(crate::theme::current().close_hover)))
                    .child("▸"),
            )
    }
}
