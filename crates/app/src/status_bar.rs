//! Статус-бар (3.5/D8.5): слева транзиентное сообщение ИЛИ кодировка,
//! переводы строк, позиция ГЛАВНОГО курсора, число курсоров, язык;
//! справа — «notepad+». Обновление — на каждый cx.notify().

use gpui::{div, prelude::*, px, rgb};

use crate::geometry::STATUS_H;
use crate::theme::{ACCENT, STATUS_DIM, STATUS_FG};
use crate::workspace::Workspace;

impl Workspace {
    /// Текст левой половины статус-бара (для рендера и тестов).
    pub(crate) fn status_bar_text(&self) -> String {
        if let Some(msg) = &self.status_message {
            return msg.clone();
        }
        if let Some(tab) = self.model.active() {
            let (line, col) = (tab.cursor_line() + 1, tab.cursor_col() + 1);
            let extra = if tab.cursors.len() > 1 {
                format!(", {} cursors", tab.cursors.len())
            } else {
                String::new()
            };
            return format!(
                "{}  |  {}  |  Ln {}, Col {}{}  |  {}",
                tab.doc.encoding().label(),
                tab.doc.line_endings().dominant().label(),
                line,
                col,
                extra,
                crate::language::language_for_path(tab.path.as_deref()),
            );
        }
        "notepad+".to_string()
    }

    pub(crate) fn render_status_bar(&self) -> gpui::Div {
        let left = self.status_bar_text();
        div()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .h(px(STATUS_H))
            .flex_shrink_0()
            .bg(rgb(ACCENT))
            .text_color(rgb(STATUS_FG))
            .text_size(px(12.0))
            .child(div().child(left))
            .child(div().text_color(rgb(STATUS_DIM)).child("notepad+"))
    }
}
