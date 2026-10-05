//! Overlay-диалог перехода к строке (Cmd+G, 4.2, D9.2): ввод цифр,
//! Enter — прыжок (+ скролл через мапу видимых), Escape — отмена.
//!
//! Ловушка gpui 0.2.2: диалог — ребёнок КОРНЕВОГО div (вне поддерева
//! редактора), иначе его on_key_down всплывает в handle_key редактора.

use gpui::{Context, KeyDownEvent, Window, div, prelude::*, px, rgb};

use crate::actions::GoToLine;
use crate::theme::{CLOSE_HOVER, DIM, TABBAR, TEXT};
use crate::workspace::Workspace;

impl Workspace {
    pub(crate) fn on_go_to_line(&mut self, _: &GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.active().is_some() {
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
                window.focus(&self.focus);
                if let Some(no) = target {
                    let no = no.max(1);
                    if let Some(tab) = self.model.active_mut() {
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
                window.focus(&self.focus);
            }
            _ => {}
        }
        cx.notify();
    }

    /// Overlay-диалог Cmd+G: рендерится как ребёнок КОРНЯ (вне
    /// поддерева редактора) — фокус в диалоге, клавиши не попадают в
    /// `handle_key` (D9.2). Позиция/размер (top 44, left 340, ширина
    /// 280) подобраны вручную: над редактором, правее панели.
    pub(crate) fn render_goto_overlay(&self, cx: &Context<Self>) -> gpui::Div {
        let input = self.goto_input.clone().unwrap_or_default();
        div()
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
            .bg(rgb(TABBAR))
            .border_1()
            .border_color(rgb(CLOSE_HOVER))
            .rounded_md()
            .text_size(px(12.0))
            .shadow_md()
            .track_focus(&self.goto_focus)
            .on_key_down(cx.listener(Self::handle_goto_key))
            .child(div().text_color(rgb(DIM)).child("Go to line:"))
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(if input.is_empty() { DIM } else { TEXT }))
                    .child(if input.is_empty() {
                        "line number…".to_string()
                    } else {
                        input
                    }),
            )
    }
}
