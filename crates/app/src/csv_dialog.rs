//! Overlay настроек CSV (D12.6): разделитель полей и символ
//! квотирования. Открывается автоматически при открытии CSV-файла без
//! сохранённого выбора (`.settings/workspace.toml`) и кликом по
//! CSV-сегменту статус-бара (паттерн выбора языка 5.1).
//!
//! Выбор чипа применяется НЕМЕДЛЕННО (живой предпросмотр колонок) и
//! персистится в настройках воркспейса; Esc/клик мимо — закрыть,
//! текущие настройки остаются. Диалог не модален по данным: целился в
//! вкладку на момент открытия, применяется к АКТИВНОЙ CSV-вкладке.

use editor_core::CsvOptions;
use gpui::{Context, KeyDownEvent, MouseButton, Window, div, prelude::*, px, rgb};

use crate::workspace::Workspace;

/// Кандидаты разделителя: авто (детект по строке), `,` `;` таб `|`.
pub(crate) const CSV_DELIMITERS: &[Option<char>] =
    &[None, Some(','), Some(';'), Some('\t'), Some('|')];
/// Кандидаты квотирования: `"`, `'`, без квотирования.
pub(crate) const CSV_QUOTES: &[Option<char>] = &[Some('"'), Some('\''), None];

impl Workspace {
    /// Клик по CSV-сегменту статус-бара: открыть настройки активной
    /// CSV-вкладки (не-CSV — no-op, сегмент не показан).
    pub(crate) fn on_status_csv_click(
        &mut self,
        _e: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.model().active().is_some_and(|t| t.is_csv()) {
            self.csv_dialog = true;
            window.focus(&self.csv_focus);
            cx.notify();
        }
    }

    /// Поставить CSV-вкладки без выбора в очередь вопроса (D12.6):
    /// точки открытия (open_paths, восстановление сессии, NOTEPAD_OPEN).
    /// Сохранённый выбор воркспейса спрашивать не нужно. Bench не спрашивает.
    pub fn queue_csv_ask(&mut self) {
        if crate::bench::enabled() {
            return;
        }
        for pi in 0..self.panes.len() {
            for i in 0..self.panes[pi].model.len() {
                let tab = &self.panes[pi].model.tabs()[i];
                let needs = tab.is_csv()
                    && !tab.csv_asked
                    && tab
                        .path
                        .as_deref()
                        .is_none_or(|p| self.saved_csv_options(p).is_none());
                if needs && !self.pending_csv_tabs.contains(&(pi, i)) {
                    self.pending_csv_tabs.push((pi, i));
                }
            }
        }
    }

    /// Спросить настройки следующего CSV из очереди: активирует
    /// вкладку, префилл — детект по содержимому (детект остаётся
    /// выбором вкладки: закрывать без правок = согласиться с детектом).
    /// Закрытие диалога продолжает очередь. Очередь могла собраться до
    /// открытия воркспейса (сессия) — сохранённый выбор перепроверяется.
    pub fn ask_pending_csv(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if crate::bench::enabled() || self.csv_dialog {
            return;
        }
        while let Some((pi, i)) = self.pending_csv_tabs.first().copied() {
            self.pending_csv_tabs.remove(0);
            // Иммутабельные проверки: вкладка жива, CSV, выбор не
            // сделан и не подтянется из настроек воркспейса.
            let no_saved = self
                .panes
                .get(pi)
                .and_then(|p| p.model.tabs().get(i))
                .and_then(|t| t.path.clone())
                .is_none_or(|p| self.saved_csv_options(&p).is_none());
            if !no_saved {
                continue;
            }
            let Some(tab) = self.panes.get_mut(pi).and_then(|p| p.model.tab_mut(i)) else {
                continue;
            };
            if !tab.is_csv() || tab.csv_asked {
                continue;
            }
            tab.csv_asked = true;
            if tab.csv_options == CsvOptions::default() {
                tab.set_csv_options(tab.detect_csv_options());
            }
            self.active_pane = pi;
            self.panes[pi].model.activate(i);
            self.csv_dialog = true;
            window.focus(&self.csv_focus);
            cx.notify();
            return;
        }
    }

    /// Применить выбор чипа (клик): настройки активной CSV-вкладки +
    /// персист в .settings (выбор переживает закрытие файла, D12.6).
    /// Диалог остаётся открытым — живой предпросмотр до закрытия.
    pub(crate) fn apply_csv_choice(
        &mut self,
        options: CsvOptions,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.model_mut().active_mut() {
            tab.csv_asked = true;
            tab.set_csv_options(options);
        }
        self.persist_workspace_marks();
        cx.notify();
    }

    /// Клавиши overlay: Escape — закрыть (выбранное уже применено).
    pub(crate) fn handle_csv_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key.as_str() == "escape" {
            self.close_csv_dialog(window, cx);
        }
    }

    /// Закрыть overlay; следом — следующий вопрос очереди открытия.
    pub(crate) fn close_csv_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.csv_dialog = false;
        window.focus(&self.active_focus());
        cx.notify();
        self.ask_pending_csv(window, cx);
    }

    /// Overlay настроек CSV: две строки чипов (разделитель/кавычки),
    /// выбранный чип подсвечен accent-фоном. Позиция — как у language
    /// overlay (над статус-баром); подложка закрывает по клику мимо.
    pub(crate) fn render_csv_overlay(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let tab = self.model().active();
        let current = tab
            .filter(|t| t.is_csv())
            .map(|t| t.csv_options)
            .unwrap_or_default();
        let title = tab
            .and_then(|t| t.path.as_ref())
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "CSV".to_string());

        // Строка чипов разделителя: применяются к текущим кавычкам.
        let mut delims = div().flex().flex_row().gap_1();
        for (idx, &delim) in CSV_DELIMITERS.iter().enumerate() {
            let options = CsvOptions { delimiter: delim, quote: current.quote };
            delims = delims
                .child(self.csv_chip(cx, ("csv-delim", idx), delim_label(cx, delim), options));
        }
        // Строка чипов квотирования: применяется к текущему разделителю.
        let mut quotes = div().flex().flex_row().gap_1();
        for (idx, &quote) in CSV_QUOTES.iter().enumerate() {
            let options = CsvOptions { delimiter: current.delimiter, quote };
            quotes = quotes
                .child(self.csv_chip(cx, ("csv-quote", idx), quote_label(cx, quote), options));
        }

        let overlay = div()
            .id("csv-overlay")
            .debug_selector(|| "csv-overlay".into())
            .absolute()
            .bottom(px(crate::geometry::STATUS_H + 8.0))
            .left(px(340.0))
            .w(px(320.0))
            .relative()
            .flex()
            .flex_col()
            .pb_2()
            .gap_1()
            .bg(rgb(crate::theme::current().panel))
            .border_1()
            .border_color(rgb(crate::theme::current().close_hover))
            .rounded_md()
            .shadow_md()
            .track_focus(&self.csv_focus)
            .on_key_down(cx.listener(Self::handle_csv_key))
            // Клик внутри не долетает до подложки (закрытия).
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .text_size(px(12.0))
                    .bg(rgb(crate::theme::current().tabbar))
                    .child(
                        div()
                            .text_color(rgb(crate::theme::current().status_dim))
                            .child(crate::i18n::t(cx, "csv.title")),
                    )
                    .child(div().flex_1().min_w_0().overflow_hidden().child(title)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_2()
                    .pt_2()
                    .child(csv_row_label(cx, "csv.delimiter"))
                    .child(delims)
                    .child(csv_row_label(cx, "csv.quote"))
                    .child(quotes),
            );
        // Подложка: клик мимо диалога закрывает (Esc-синоним).
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _e, window, cx| {
                    this.close_csv_dialog(window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _e, window, cx| {
                    this.close_csv_dialog(window, cx);
                }),
            )
            .child(overlay)
            .into_any_element()
    }

    /// Чип выбора: клик применяется немедленно (живой предпросмотр);
    /// выбранный подсвечен фоном accent.
    fn csv_chip(
        &self,
        cx: &Context<Self>,
        id: impl Into<gpui::ElementId>,
        label: String,
        options: CsvOptions,
    ) -> gpui::Stateful<gpui::Div> {
        let tab = self.model().active();
        let selected = tab
            .filter(|t| t.is_csv())
            .is_some_and(|t| t.csv_options == options);
        let selector = format!("csv-chip:{label}");
        let theme = crate::theme::current();
        let (chip_bg, chip_fg) = if selected {
            (theme.accent, theme.status_fg)
        } else {
            (theme.panel, theme.text)
        };
        div()
            .id(id)
            .debug_selector(move || selector.clone())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _e, _window, cx| {
                    cx.stop_propagation();
                    this.apply_csv_choice(options, cx);
                }),
            )
            .px_2()
            .py_1()
            .text_size(px(12.0))
            .rounded_sm()
            .border_1()
            .border_color(rgb(if selected { theme.accent } else { theme.close_hover }))
            .bg(rgb(chip_bg))
            .text_color(rgb(chip_fg))
            .hover(|d| d.bg(rgb(if selected { theme.accent } else { theme.close_hover })))
            .child(label)
    }
}

/// Подпись строки чипов («Разделитель:» / «Кавычки:»).
fn csv_row_label(cx: &gpui::App, key: &str) -> gpui::Div {
    div()
        .px_2()
        .text_size(px(12.0))
        .text_color(rgb(crate::theme::current().dim))
        .child(crate::i18n::t(cx, key))
}

/// Метка чипа разделителя: `None` — «Auto», таб — «Tab», прочее — символ.
pub(crate) fn delim_label(cx: &gpui::App, delim: Option<char>) -> String {
    match delim {
        None => crate::i18n::t(cx, "csv.auto"),
        Some('\t') => crate::i18n::t(cx, "csv.tab"),
        Some(c) => c.to_string(),
    }
}

/// Метка чипа квотирования: `None` — «None», прочее — символ.
pub(crate) fn quote_label(cx: &gpui::App, quote: Option<char>) -> String {
    match quote {
        None => crate::i18n::t(cx, "csv.none"),
        Some(c) => c.to_string(),
    }
}

/// Короткая метка CSV-сегмента статус-бара: «CSV: <sep> <quote>»
/// (авто/нет — локализованные слова в нижнем регистре).
pub(crate) fn csv_status_label(cx: &gpui::App, options: CsvOptions) -> String {
    let d = match options.delimiter {
        None => crate::i18n::t(cx, "csv.status_auto"),
        Some('\t') => crate::i18n::t(cx, "csv.tab"),
        Some(c) => c.to_string(),
    };
    let q = match options.quote {
        None => crate::i18n::t(cx, "csv.status_none"),
        Some(c) => c.to_string(),
    };
    format!("CSV: {d} {q}")
}
