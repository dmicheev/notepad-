//! Overlay выбора языка (5.1, паттерн goto_dialog/D9.2): список языков
//! с фильтрацией вводом, клик/Enter — применить, Escape — отмена.
//! Открывается кликом по языку в статус-баре.
//!
//! Записи: «Auto-detect» (вернуть авто), «Plain Text» (без подсветки),
//! далее языки `language_list()` (≥70, D12.3). Файл >20 МБ — выбор
//! запрещён сообщением в статус-бар (решение D12.3).
//!
//! Ловушка gpui 0.2.2 (как Cmd+G): overlay — ребёнок КОРНЕВОГО div,
//! фокус в собственном handle — клавиши не всплывают в редактор.

use gpui::{Context, KeyDownEvent, MouseButton, Window, div, prelude::*, px, rgb};

use crate::theme::{CLOSE_HOVER, DIM, PANEL, TABBAR, TEXT};
use crate::workspace::Workspace;

impl Workspace {
    /// Клик по языку в статус-баре: открыть список. Файл сверх порога
    /// БЕЗ разрешения — сначала вопрос о включении подсветки (D12.4;
    /// после согласия повторный клик открывает список выбора).
    pub(crate) fn on_status_language_click(
        &mut self,
        _e: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.model.active_index() {
            let needs_ask = self
                .model
                .tab_mut(index)
                .is_some_and(|t| t.language.over_threshold && !t.language.threshold_allowed);
            if needs_ask {
                self.pending_threshold_tabs.push(index);
                self.ask_pending_threshold(window, cx);
                return;
            }
        }
        if self.model.active().is_some() {
            self.language_input = Some(String::new());
            window.focus(&self.language_focus);
            cx.notify();
        }
    }

    /// Применить выбор языка (клик/Enter): Auto / Plain Text / язык.
    pub(crate) fn apply_language_choice(
        &mut self,
        choice: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.language_input = None;
        window.focus(&self.focus);
        if let Some(tab) = self.model.active_mut() {
            match choice {
                "Auto-detect" => tab.set_language_auto(),
                "Plain Text" => tab.set_language(None),
                name => {
                    if let Some(syntax) = crate::language::syntax_for_choice(name) {
                        tab.set_language(Some(syntax));
                    } else {
                        return; // неизвестное имя — просто закрыть
                    }
                }
            }
        }
        cx.notify();
    }

    /// Клавиши overlay: символы — фильтр, backspace, Enter — первый
    /// видимый вариант, Escape — отмена. Висит на overlay (фокус —
    /// редактор клавиш не получает).
    pub(crate) fn handle_language_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        let Some(input) = &mut self.language_input else { return };
        match ks.key.as_str() {
            "enter" => {
                let choice = self.filtered_languages().into_iter().next().map(str::to_string);
                if let Some(choice) = choice {
                    self.apply_language_choice(&choice, window, cx);
                }
            }
            "escape" => {
                self.language_input = None;
                window.focus(&self.focus);
            }
            "backspace" => {
                input.pop();
            }
            _ => {
                if let Some(ch) = ks.key_char.as_deref() {
                    for c in ch.chars() {
                        if !c.is_control() && input.len() < 32 {
                            input.push(c);
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    /// Отфильтрованный список для overlay: спец-записи + языки,
    /// содержащие ввод (регистронезависимо).
    pub(crate) fn filtered_languages(&self) -> Vec<&'static str> {
        let query = self
            .language_input
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        let matches = |name: &str| {
            query.is_empty() || name.to_ascii_lowercase().contains(&query)
        };
        let mut out = vec!["Auto-detect", "Plain Text"];
        out.extend(
            crate::language::language_list()
                .into_iter()
                .filter(|n| matches(n)),
        );
        // Пустой фильтр без совпадений — хотя бы спец-записи (Enter по
        // ним валиден: авто/текст).
        let _ = matches; // уже применён выше
        out
    }

    /// Overlay списка языков: ребёнок КОРНЯ (как Cmd+G, D9.2), над
    /// статус-баром. Позиция/размер (bottom 32, left 340, ширина 320,
    /// высота 320) подобраны вручную: над статус-баром, правее панели.
    pub(crate) fn render_language_overlay(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let input = self.language_input.clone().unwrap_or_default();
        let items = self.filtered_languages();
        let mut list = div().flex().flex_col();
        for (li, name) in items.iter().enumerate() {
            let name = *name;
            list = list.child(
                div()
                    .id(("lang-item", li))
                    .debug_selector(move || format!("lang-item:{name}").into())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _e, window, cx| {
                            cx.stop_propagation();
                            this.apply_language_choice(name, window, cx);
                        }),
                    )
                    .px_2()
                    .py_1()
                    .text_size(px(12.0))
                    .hover(|d| d.bg(rgb(CLOSE_HOVER)))
                    .child(name),
            );
        }
        div()
            .id("language-overlay")
            .absolute()
            .bottom(px(crate::geometry::STATUS_H + 8.0))
            .left(px(340.0))
            .w(px(320.0))
            .max_h(px(320.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .pb_2()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(CLOSE_HOVER))
            .rounded_md()
            .shadow_md()
            .track_focus(&self.language_focus)
            .on_key_down(cx.listener(Self::handle_language_key))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .text_size(px(12.0))
                    .bg(rgb(TABBAR))
                    .child(div().text_color(rgb(DIM)).child("Language:"))
                    .child(
                        div()
                            .flex_1()
                            .text_color(rgb(if input.is_empty() { DIM } else { TEXT }))
                            .child(if input.is_empty() {
                                "type to filter…".to_string()
                            } else {
                                input
                            }),
                    ),
            )
            .child(list)
            .into_any_element()
    }
}
