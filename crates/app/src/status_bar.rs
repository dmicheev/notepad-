//! Статус-бар: СЛЕВА стабильный блок — кодировка, переводы строк,
//! позиция ГЛАВНОГО курсора, число курсоров, язык (кликабельный —
//! оверлей выбора языка 5.1); СПРАВА — транзиентное сообщение (пока
//! висит — вместо «notepad+»). Сообщение НЕ подменяет левый блок
//! (регрессия: «Syntax highlighting is off» вытеснял позицию/язык);
//! блоки разделяются justify_between — не перекрываются. Обновление —
//! на каждый cx.notify().

use gpui::{div, prelude::*, px, rgb};

use crate::geometry::STATUS_H;
use crate::workspace::Workspace;

impl Workspace {
    /// Левые части статус-бара (единый источник для текста и рендера):
    /// префикс «кодировка | переводы | Ln/Col | …» + метка CSV-настроек
    /// (кликабельная зона, D12.6; только CSV-вкладки) + метка языка
    /// (кликабельная зона). `None` — вкладок нет.
    fn status_parts(&self, cx: &gpui::App) -> Option<(String, Option<String>, &'static str)> {
        let tab = self.model().active()?;
        let (line, col) = (tab.cursor_line() + 1, tab.cursor_col() + 1);
        let extra = if tab.cursors.len() > 1 {
            format!(
                ", {}",
                crate::i18n::tf(cx, "status.cursors", &[("n", &tab.cursors.len().to_string())])
            )
        } else {
            String::new()
        };
        let csv = tab
            .is_csv()
            .then(|| crate::csv_dialog::csv_status_label(cx, tab.csv_options));
        Some((
            format!(
                "{}  |  {}  |  Ln {}, Col {}{}  |  ",
                tab.doc.encoding().label(),
                tab.doc.line_endings().dominant().label(),
                line,
                col,
                extra,
            ),
            csv,
            tab.language_label(),
        ))
    }

    /// Текст ЛЕВОГО блока — стабильный, от сообщений не зависит
    /// (тестовая поверхность; рендер строит блоки сам).
    #[cfg(test)]
    pub(crate) fn status_bar_text(&self, cx: &gpui::App) -> String {
        match self.status_parts(cx) {
            Some((prefix, csv, language)) => match csv {
                Some(csv) => format!("{prefix}{csv}  |  {language}"),
                None => format!("{prefix}{language}"),
            },
            None => "notepad+".to_string(),
        }
    }

    pub(crate) fn render_status_bar(&self, cx: &gpui::Context<Self>) -> gpui::Div {
        // Слева — всегда стандартный блок (CSV-настройки и язык
        // кликабельны, D12.6/5.1); справа — транзиентное сообщение или
        // «notepad+».
        let (prefix, csv, language) = self
            .status_parts(cx)
            .unwrap_or_else(|| ("notepad+".to_string(), None, ""));
        let message = self.status_message.clone();
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .px_3()
            .h(px(STATUS_H))
            .flex_shrink_0()
            .bg(rgb(crate::theme::current().accent))
            .text_color(rgb(crate::theme::current().status_fg))
            .text_size(px(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .overflow_hidden()
                    .child(div().child(prefix))
                    .when_some(csv, |d, csv| {
                        d.child(
                            div()
                                .id("status-csv")
                                .debug_selector(|| "status-csv".into())
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(Self::on_status_csv_click),
                                )
                                .hover(|d| d.bg(rgb(crate::theme::current().status_lang_hover)))
                                .child(csv),
                        )
                        .child(div().child("  |  "))
                    })
                    .when(!language.is_empty(), |d| {
                        d.child(
                            div()
                                .id("status-language")
                                .debug_selector(|| "status-language".into())
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(Self::on_status_language_click),
                                )
                                .hover(|d| d.bg(rgb(crate::theme::current().status_lang_hover)))
                                .child(language),
                        )
                    }),
            )
            .child(
                div()
                    .id("status-message")
                    .debug_selector(|| "status-message".into())
                    .flex_shrink_0()
                    .min_w_0()
                    .max_w(gpui::relative(0.6))
                    .overflow_hidden()
                    .text_color(rgb(crate::theme::current().status_dim))
                    .child(message.unwrap_or_else(|| "notepad+".to_string())),
            )
    }
}
