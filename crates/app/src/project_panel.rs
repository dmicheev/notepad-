//! Левая панель «EXPLORER» (3.1): заглушка дерева воркспейса (группа
//! 7) + секция «BOOKMARKS (N)» активного файла (4.4/D9.4) — записи
//! «номер + содержимое строки», клик — переход.

use gpui::{Context, div, prelude::*, px, rgb};

use crate::geometry::PANEL_W;
use crate::theme::{ACCENT, CLOSE_HOVER, DIM, PANEL, PANEL_HEADER, PANEL_HINT, TAB_INACTIVE, TEXT};
use crate::workspace::Workspace;

impl Workspace {
    /// Панель «Проект»: заглушка дерева (группа 7) + закладки.
    pub(crate) fn render_project_panel(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        // Панель «Проект» — заглушка (дерево воркспейса — группа 7);
        // внизу — панель закладок активного файла (4.4/D9.4).
        let bookmarks = self
            .model
            .active()
            .map(|t| {
                t.bookmarked_lines()
                    .into_iter()
                    .map(|line| (line, t.doc.line_content(line)))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut panel = div()
            .id("project-panel")
            .w(px(PANEL_W))
            .flex_shrink_0()
            .bg(rgb(PANEL))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(11.0))
                    .text_color(rgb(PANEL_HEADER))
                    .child("EXPLORER"),
            )
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(12.0))
                    .text_color(rgb(DIM))
                    .child("Project workspace tree"),
            )
            .child(
                div()
                    .px_3()
                    .text_size(px(11.0))
                    .text_color(rgb(PANEL_HINT))
                    .child("(coming in group 7 — workspace)"),
            );
        if !bookmarks.is_empty() {
            let mut section = div()
                .mt_2()
                .border_t_1()
                .border_color(rgb(CLOSE_HOVER))
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .text_size(px(11.0))
                        .text_color(rgb(PANEL_HEADER))
                        .child(format!("BOOKMARKS ({})", bookmarks.len())),
                );
            for (line, content) in bookmarks {
                section = section.child(
                    div()
                        .id(("bookmark-entry", line))
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            this.goto_bookmark(line, cx);
                        }))
                        .flex()
                        .gap_2()
                        .px_3()
                        .py_1()
                        .text_size(px(11.0))
                        .text_color(rgb(TEXT))
                        .hover(|d| d.bg(rgb(CLOSE_HOVER)))
                        .child(
                            div()
                                .text_color(rgb(ACCENT))
                                .flex_shrink_0()
                                .child(format!("{}", line + 1)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis()
                                .text_color(rgb(TAB_INACTIVE))
                                .child(if content.is_empty() { " ".to_string() } else { content }),
                        ),
                );
            }
            panel = panel.child(section);
        }
        // Секция live-статистики производительности и памяти (D11.8).
        panel = panel.child(self.render_perf_section());
        panel
    }
}
