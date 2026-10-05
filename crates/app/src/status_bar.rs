//! Статус-бар (3.5/D8.5 + 5.1): слева транзиентное сообщение ИЛИ
//! кодировка, переводы строк, позиция ГЛАВНОГО курсора, число курсоров,
//! язык (кликабельный — оверлей выбора языка 5.1); справа — «notepad+».
//! Обновление — на каждый cx.notify().

use gpui::{div, prelude::*, px, rgb};

use crate::geometry::STATUS_H;
use crate::theme::{ACCENT, STATUS_DIM, STATUS_FG, STATUS_LANG_HOVER};
use crate::workspace::Workspace;

impl Workspace {
    /// Левые части статус-бара БЕЗ транзиентного сообщения (единый
    /// источник для текста и рендера): префикс «кодировка | переводы
    /// | Ln/Col | …» + метка языка (кликабельная зона). `None` —
    /// вкладок нет.
    fn status_parts(&self) -> Option<(String, &'static str)> {
        let tab = self.model.active()?;
        let (line, col) = (tab.cursor_line() + 1, tab.cursor_col() + 1);
        let extra = if tab.cursors.len() > 1 {
            format!(", {} cursors", tab.cursors.len())
        } else {
            String::new()
        };
        Some((
            format!(
                "{}  |  {}  |  Ln {}, Col {}{}  |  ",
                tab.doc.encoding().label(),
                tab.doc.line_endings().dominant().label(),
                line,
                col,
                extra,
            ),
            tab.language_label(),
        ))
    }

    /// Текст левой половины статус-бара (для рендера и тестов).
    pub(crate) fn status_bar_text(&self) -> String {
        if let Some(msg) = &self.status_message {
            return msg.clone();
        }
        match self.status_parts() {
            Some((prefix, language)) => format!("{prefix}{language}"),
            None => "notepad+".to_string(),
        }
    }

    pub(crate) fn render_status_bar(&self, cx: &gpui::Context<Self>) -> gpui::Div {
        // Язык — кликабельная зона (5.1): открывает оверлей выбора.
        // Пока висит транзиентное сообщение — язык недоступен (весь
        // левый блок заменён сообщением, D8.5).
        let (prefix, language) = match (self.status_message.is_none(), self.status_parts()) {
            (true, Some(parts)) => parts,
            _ => (self.status_bar_text(), ""),
        };
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .overflow_hidden()
                    .child(div().child(prefix))
                    .when(!language.is_empty(), |d| {
                        d.child(
                            div()
                                .id("status-language")
                                .debug_selector(|| "status-language".into())
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(Self::on_status_language_click),
                                )
                                .hover(|d| d.bg(rgb(STATUS_LANG_HOVER)))
                                .child(language),
                        )
                    }),
            )
            .child(div().text_color(rgb(STATUS_DIM)).child("notepad+"))
    }
}
