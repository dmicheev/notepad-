//! Overlay-диалог перехода к строке (Cmd+G, 4.2, D9.2): ввод цифр,
//! Enter — прыжок (+ скролл через мапу видимых), Escape — отмена.
//!
//! Ловушка gpui 0.2.2: диалог — ребёнок КОРНЕВОГО div (вне поддерева
//! редактора), иначе его on_key_down всплывает в handle_key редактора.

use gpui::{Context, KeyDownEvent, Window, div, prelude::*, px, rgb};

use crate::actions::GoToLine;
use crate::workspace::Workspace;

impl Workspace {
    pub(crate) fn on_go_to_line(&mut self, _: &GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        if self.model().active().is_some() {
            self.goto_input = Some(String::new());
            window.focus(&self.goto_focus);
            cx.notify();
        }
    }

    /// Клавиши overlay-диалога: разбор буфера — overlay_input (цифры,
    /// лимит 9), Enter — переход, Escape — отмена. Висит на диалоге
    /// (фокус диалога — редактор клавиш не получает).
    pub(crate) fn handle_goto_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let outcome = self
            .goto_input
            .as_mut()
            .map(|input| {
                crate::overlay_input::feed(input, event, 9, |c| c.is_ascii_digit())
            });
        match outcome {
            Some(crate::overlay_input::InputKey::Submit) => {
                let target = self.goto_input.as_deref().unwrap_or("").trim().parse::<usize>().ok();
                self.goto_input = None;
                window.focus(&self.active_focus());
                if let Some(no) = target {
                    let no = no.max(1);
                    if let Some(tab) = self.model_mut().active_mut() {
                        let line = (no - 1).min(tab.doc.len_lines().saturating_sub(1));
                        // Строка внутри свёрнутого тела → каретка на заголовок.
                        let line = tab.visible_line_or_header(line);
                        tab.set_caret_at(line, 0);
                        self.scroll_to_line(line);
                    }
                }
            }
            Some(crate::overlay_input::InputKey::Cancel) => {
                self.goto_input = None;
                window.focus(&self.active_focus());
            }
            _ => {}
        }
        cx.notify();
    }

    /// Overlay-диалог Cmd+G: рендерится как ребёнок КОРНЯ (вне
    /// поддерева редактора) — фокус в диалоге, клавиши не попадают в
    /// `handle_key` (D9.2). Позиция/размер (top 44, left 340, ширина
    /// 280) подобраны вручную: над редактором, правее панели.
    /// Подложка на всё окно: клик мимо диалога закрывает (Esc-синоним);
    /// клик внутри гасится (stop_propagation) — подложка не съедает.
    pub(crate) fn render_goto_overlay(&self, cx: &Context<Self>) -> gpui::Div {
        use gpui::MouseButton;
        let input = self.goto_input.clone().unwrap_or_default();
        let dialog = div()
            .absolute()
            // Позиция поверх области редактора (подобрана вручную).
            .top(px(44.0))
            .left(px(340.0))
            .w(px(280.0))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .bg(rgb(crate::theme::current().tabbar))
            .border_1()
            .border_color(rgb(crate::theme::current().close_hover))
            .rounded_md()
            .text_size(px(12.0))
            .shadow_md()
            .track_focus(&self.goto_focus)
            .on_key_down(cx.listener(Self::handle_goto_key))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(div().text_color(rgb(crate::theme::current().dim)).child(crate::i18n::t(cx, "goto.label")))
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(if input.is_empty() { crate::theme::current().dim } else { crate::theme::current().text }))
                    .child(if input.is_empty() {
                        crate::i18n::t(cx, "goto.placeholder")
                    } else {
                        input
                    }),
            );
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _e, window, cx| {
                    this.goto_input = None;
                    window.focus(&this.active_focus());
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _e, window, cx| {
                    this.goto_input = None;
                    window.focus(&this.active_focus());
                    cx.notify();
                }),
            )
            .child(dialog)
    }
}
