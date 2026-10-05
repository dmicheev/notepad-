//! Левая панель «EXPLORER» (3.1): дерево воркспейса (группа 7,
//! tree_panel.rs::render_tree_section) + секция «BOOKMARKS (N)»
//! активного файла (4.4/D9.4) — записи «номер + содержимое строки»,
//! клик — переход.

use gpui::{Context, div, prelude::*, px, rgb};

use crate::geometry::PANEL_W;
use crate::theme::{ACCENT, CLOSE_HOVER, PANEL, PANEL_HEADER, TAB_INACTIVE, TEXT};
use crate::workspace::Workspace;

impl Workspace {
    /// Панель «Проект»: дерево воркспейса (7.1) + закладки.
    /// `&mut self` — дерево лениво читает/кэширует детей раскрытых
    /// папок при построении строк (fs_tree.rs).
    pub(crate) fn render_project_panel(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        // Закладки активного файла (4.4/D9.4).
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
        // Дерево воркспейса (7.1) — до закладок; &mut self заимствован
        // выше только для чтения модели вкладок (борrows disjoint).
        let tree_section = self.render_tree_section(cx);
        // Скроллимое тело панели: track_scroll даёт handle полосе
        // (scrollbar.rs), сама полоса — оверлей обёртки ниже.
        let mut body = div()
            .id("project-panel")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.panel_scroll)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(11.0))
                    .text_color(rgb(PANEL_HEADER))
                    .child("EXPLORER"),
            )
            .child(tree_section.flex_shrink_0());
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
            body = body.child(section);
        }
        // Секция результатов поиска Tabs/Folder (6.3/6.4, D13) — по
        // образцу BOOKMARKS; только при непустых результатах/скане.
        if let Some(search_section) = self.render_search_section(cx) {
            body = body.child(search_section);
        }
        // Секция live-статистики производительности и памяти (D11.8).
        body = body.child(self.render_perf_section());
        // Обёртка панели: размеры/фон здесь, тело скроллится внутри,
        // полоса — absolute-оверлей правого края; move/up обёртки
        // ведут драг ползунка (scrollbar.rs).
        div()
            .id("project-panel-zone")
            .relative()
            .w(px(PANEL_W))
            .flex_shrink_0()
            .bg(rgb(PANEL))
            .flex()
            .flex_col()
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
            .child(body)
            .child(self.render_vertical_scrollbar(crate::scrollbar::ScrollTarget::Panel, cx))
    }
}
