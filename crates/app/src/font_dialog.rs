//! Overlay настройки шрифта (8.3/D15.3, паттерн language_dialog):
//! список системных семейств с фильтрацией + ряд размеров; клик/Enter —
//! применить НЕМЕДЛЕННО (перерисовка всех панелей, char_advance
//! перемеряется), Escape — закрыть. Открывается из меню View → Font….
//!
//! Семейства — `text_system().all_font_names()` (кэш в Workspace,
//! собирается при первом открытии). Размеры — набор фиксированных
//! чипов + зум Cmd+=/Cmd+-/Cmd+0 (8.5).

use gpui::{Context, KeyDownEvent, MouseButton, Window, div, prelude::*, px, rgb};

use crate::workspace::Workspace;

/// Набор размеров-чипов (px).
const SIZES: [f32; 9] = [10.0, 11.0, 12.0, 13.0, 14.0, 16.0, 18.0, 20.0, 24.0];

impl Workspace {
    /// Меню View → Font…: открыть overlay (список семейств + размеры).
    pub(crate) fn on_open_font_dialog(
        &mut self,
        _: &crate::actions::OpenFontDialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.all_fonts.is_empty() {
            self.all_fonts = window.text_system().all_font_names();
            self.all_fonts.sort();
        }
        self.font_dialog = Some(String::new());
        window.focus(&self.font_focus);
        cx.notify();
    }

    /// Зум: Cmd+= (ZoomIn) / Cmd+- (ZoomOut) / Cmd+0 (ZoomReset).
    pub(crate) fn on_zoom_in(&mut self, _: &crate::actions::ZoomIn, _w: &mut Window, cx: &mut Context<Self>) {
        self.zoom_font(1, cx);
    }

    pub(crate) fn on_zoom_out(&mut self, _: &crate::actions::ZoomOut, _w: &mut Window, cx: &mut Context<Self>) {
        self.zoom_font(-1, cx);
    }

    pub(crate) fn on_zoom_reset(&mut self, _: &crate::actions::ZoomReset, _w: &mut Window, cx: &mut Context<Self>) {
        self.zoom_font(0, cx);
    }

    /// Применить семейство (клик/Enter по списку; размер не меняется).
    pub(crate) fn apply_font_family(
        &mut self,
        family: &str,
        cx: &mut Context<Self>,
    ) {
        let mut font = self.font.clone();
        font.family = family.to_string();
        self.apply_font(font, cx);
    }

    /// Применить размер (клик по чипу; семейство не меняется).
    pub(crate) fn apply_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let mut font = self.font.clone();
        font.size = size;
        self.apply_font(font, cx);
    }

    /// Клавиши overlay: фильтр (overlay_input), Enter — первое
    /// отфильтрованное семейство, Escape — закрыть.
    pub(crate) fn handle_font_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let outcome = self
            .font_dialog
            .as_mut()
            .map(|input| crate::overlay_input::feed(input, event, 48, |c| !c.is_control()));
        match outcome {
            Some(crate::overlay_input::InputKey::Submit) => {
                if let Some(family) = self.filtered_font_families().into_iter().next() {
                    self.apply_font_family(&family, cx);
                }
            }
            Some(crate::overlay_input::InputKey::Cancel) => {
                self.font_dialog = None;
                window.focus(&self.active_focus());
            }
            _ => {}
        }
        cx.notify();
    }

    /// Отфильтрованные семейства: текущее первым (быстрый повторный
    /// выбор), далее содержащие ввод (регистронезависимо).
    pub(crate) fn filtered_font_families(&self) -> Vec<String> {
        let query = self
            .font_dialog
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut out: Vec<String> = self
            .all_fonts
            .iter()
            .filter(|n| query.is_empty() || n.to_ascii_lowercase().contains(&query))
            .cloned()
            .collect();
        // Текущее семейство не обязано быть в списке системных
        // (шрифт из сессии / тестовая текстовая система) — добавляем,
        // сортировка ставит его первым.
        if !out.contains(&self.font.family) {
            out.push(self.font.family.clone());
        }
        out.sort_by_key(|n| (*n != self.font.family, n.clone()));
        out
    }

    /// Overlay: заголовок с текущим шрифтом, фильтр, список семейств,
    /// ряд размеров; клики применяют немедленно.
    pub(crate) fn render_font_overlay(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let input = self.font_dialog.clone().unwrap_or_default();
        let current_size = self.font.size;
        let families = self.filtered_font_families();
        let mut list = div().flex().flex_col();
        for (fi, name) in families.iter().enumerate() {
            let name = name.clone();
            let is_current = name == self.font.family;

            list = list.child(
                div()
                    .id(("font-item", fi))
                    .debug_selector({
                        let name = name.clone();
                        move || format!("font-item:{name}")
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        {
                            let name = name.clone();
                            cx.listener(move |this, _e, _window, cx| {
                                cx.stop_propagation();
                                this.apply_font_family(&name, cx);
                            })
                        },
                    )
                    .px_2()
                    .py_1()
                    .text_size(px(12.0))
                    .text_color(rgb(if is_current {
                        crate::theme::current().accent
                    } else {
                        crate::theme::current().text
                    }))
                    .hover(|d| d.bg(rgb(crate::theme::current().close_hover)))
                    .child(name.clone()),
            );
        }
        // Ряд размеров-чипов: текущий выделен цветом.
        let mut sizes_row = div().flex().flex_wrap().gap_1().px_2().py_2();
        for size in SIZES {
            let is_current = (size - current_size).abs() < 0.01;
            sizes_row = sizes_row.child(
                div()
                    .id(("font-size", size.to_bits()))
                    .debug_selector(move || format!("font-size:{size}"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _e, _window, cx| {
                            cx.stop_propagation();
                            this.apply_font_size(size, cx);
                        }),
                    )
                    .px_2()
                    .py_1()
                    .text_size(px(12.0))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(if is_current {
                        crate::theme::current().accent
                    } else {
                        crate::theme::current().close_hover
                    }))
                    .text_color(rgb(if is_current {
                        crate::theme::current().accent
                    } else {
                        crate::theme::current().text
                    }))
                    .child(format!("{size}")),
            );
        }
        div()
            .id("font-overlay")
            .absolute()
            .bottom(px(crate::geometry::STATUS_H + 8.0))
            .left(px(340.0))
            .w(px(320.0))
            .max_h(px(340.0))
            .relative()
            .flex()
            .flex_col()
            .pb_2()
            .bg(rgb(crate::theme::current().panel))
            .border_1()
            .border_color(rgb(crate::theme::current().close_hover))
            .rounded_md()
            .shadow_md()
            .track_focus(&self.font_focus)
            .on_key_down(cx.listener(Self::handle_font_key))
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
                    .child(div().text_color(rgb(crate::theme::current().dim)).child(crate::i18n::t(cx, "font.label")))
                    .child(
                        div()
                            .flex_1()
                            .text_color(rgb(if input.is_empty() {
                                crate::theme::current().dim
                            } else {
                                crate::theme::current().text
                            }))
                            .child(if input.is_empty() {
                                crate::i18n::tf(
                                    cx,
                                    "font.filter_placeholder",
                                    &[
                                        ("family", &self.font.family),
                                        ("size", &self.font.size.to_string()),
                                    ],
                                )
                            } else {
                                input
                            }),
                    ),
            )
            .child(
                list.id("font-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll(),
            )
            .child(sizes_row)
            .into_any_element()
    }
}
