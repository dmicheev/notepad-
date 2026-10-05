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
        let pi = self.active_pane;
        if let Some(index) = self.panes[pi].model.active_index() {
            let needs_ask = self.panes[pi]
                .model
                .tab_mut(index)
                .is_some_and(|t| t.language.over_threshold && !t.language.threshold_allowed);
            if needs_ask {
                self.pending_threshold_tabs.push((pi, index));
                self.ask_pending_threshold(window, cx);
                return;
            }
        }
        if self.model().active().is_some() {
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
        window.focus(&self.active_focus());
        if let Some(tab) = self.model_mut().active_mut() {
            if choice == crate::i18n::t(cx, "lang.auto_detect") {
                tab.set_language_auto();
            } else if choice == "Plain Text" {
                tab.set_language(None);
            } else if let Some(syntax) = crate::language::syntax_for_choice(choice) {
                tab.set_language(Some(syntax));
            } else {
                return; // неизвестное имя — просто закрыть
            }
        }
        cx.notify();
    }

    /// Клавиши overlay: разбор буфера — overlay_input (без контролей,
    /// лимит 32), Enter — первый видимый вариант, Escape — отмена.
    /// Висит на overlay (фокус — редактор клавиш не получает).
    pub(crate) fn handle_language_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let outcome = self
            .language_input
            .as_mut()
            .map(|input| crate::overlay_input::feed(input, event, 32, |c| !c.is_control()));
        match outcome {
            Some(crate::overlay_input::InputKey::Submit) => {
                let choice = self.filtered_languages(cx).into_iter().next();
                if let Some(choice) = choice {
                    self.apply_language_choice(&choice, window, cx);
                }
            }
            Some(crate::overlay_input::InputKey::Cancel) => {
                self.language_input = None;
                window.focus(&self.active_focus());
            }
            _ => {}
        }
        cx.notify();
    }

    /// Отфильтрованный список для overlay: спец-записи всегда видны
    /// (Enter по ним валиден — авто/текст), далее языки, содержащие
    /// ввод (регистронезависимо).
    pub(crate) fn filtered_languages(&self, cx: &gpui::App) -> Vec<String> {
        let query = self
            .language_input
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        let matches = |name: &str| {
            query.is_empty() || name.to_ascii_lowercase().contains(&query)
        };
        let mut out: Vec<String> = vec![
            crate::i18n::t(cx, "lang.auto_detect"),
            "Plain Text".to_string(),
        ];
        out.extend(
            crate::language::language_list()
                .into_iter()
                .filter(|n| matches(n))
                .map(str::to_string),
        );
        out
    }

    /// Overlay списка языков: ребёнок КОРНЯ (как Cmd+G, D9.2), над
    /// статус-баром. Позиция/размер (bottom 32, left 340, ширина 320,
    /// высота 320) подобраны вручную: над статус-баром, правее панели.
    /// Скроллится внутренний список (track_scroll), полоса — оверлей
    /// обёртки (scrollbar.rs); move/up обёртки ведут драг ползунка.
    pub(crate) fn render_language_overlay(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let input = self.language_input.clone().unwrap_or_default();
        let items = self.filtered_languages(cx);
        let mut list = div().flex().flex_col();
        for (li, name) in items.iter().enumerate() {
            let name = name.clone();
            let name_for_sel = name.clone();
            let name_for_click = name.clone();
            list = list.child(
                div()
                    .id(("lang-item", li))
                    .debug_selector(move || format!("lang-item:{name_for_sel}"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _e, window, cx| {
                            cx.stop_propagation();
                            this.apply_language_choice(&name_for_click, window, cx);
                        }),
                    )
                    .px_2()
                    .py_1()
                    .text_size(px(12.0))
                    .hover(|d| d.bg(rgb(crate::theme::current().close_hover)))
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
            .relative()
            .flex()
            .flex_col()
            .pb_2()
            .bg(rgb(crate::theme::current().panel))
            .border_1()
            .border_color(rgb(crate::theme::current().close_hover))
            .rounded_md()
            .shadow_md()
            .track_focus(&self.language_focus)
            .on_key_down(cx.listener(Self::handle_language_key))
            .on_mouse_move(cx.listener(
                |this, event: &gpui::MouseMoveEvent, window, cx| {
                    this.on_scrollbar_drag_move(event, window, cx);
                },
            ))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseUpEvent, window, cx| {
                    this.on_scrollbar_drag_up(event, window, cx);
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .text_size(px(12.0))
                    .bg(rgb(crate::theme::current().tabbar))
                    .child(div().text_color(rgb(crate::theme::current().dim)).child(crate::i18n::t(cx, "lang.label")))
                    .child(
                        div()
                            .flex_1()
                            .text_color(rgb(if input.is_empty() { crate::theme::current().dim } else { crate::theme::current().text }))
                            .child(if input.is_empty() {
                                crate::i18n::t(cx, "lang.filter_placeholder")
                            } else {
                                input
                            }),
                    ),
            )
            .child(
                list.id("language-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.language_scroll),
            )
            .child(self.render_vertical_scrollbar(crate::scrollbar::ScrollTarget::Language, cx))
            .into_any_element()
    }
}
