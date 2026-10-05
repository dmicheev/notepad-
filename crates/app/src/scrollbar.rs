//! Полосы прокрутки (overlay): gpui 0.2.2 НЕ рисует полосы сам —
//! `scrollbar_width` только резервирует место в layout (taffy), а
//! `overflow_*_scroll` / `uniform_list` дают скролл без индикатора.
//! Полосы рисуются здесь как ABSOLUTE-оверлеи поверх скроллящегося
//! контейнера (паттерн липкой шапки CSV D12.5 — якорь `relative`).
//!
//! Охват: редактор (uniform_list, base_handle), левая панель,
//! оверлей выбора языка (div + `track_scroll`), таб-бар
//! (горизонтальная). Геометрия — из `ScrollHandle` (bounds /
//! max_offset / offset), значения с прошлого prepaint (ловушка
//! render-до-paint — та же, что у липкой шапки): полоса появляется
//! со второго кадра; старт закрывает отложенный notify прогрева
//! подсветки (50 мс, D12.1).
//!
//! Драг ползунка: move/up слушает КОНТЕЙНЕР зоны скролла (приём
//! выделения текста 4.2 — up прилетает и вне ползунка). Пропущенный
//! up лечится проверкой `pressed_button`: move без зажатой кнопки
//! при живом драге завершает его.

use gpui::{AnyElement, Context, MouseButton, ScrollHandle, div, point, prelude::*, px, rgb};

use crate::workspace::Workspace;

/// Ширина дорожки (px).
pub(crate) const TRACK_W: f32 = 10.0;
/// Минимальная длина ползунка (px) — иначе в длинных файлах он
/// становится нецепляемым.
pub(crate) const THUMB_MIN_LEN: f32 = 28.0;

/// Какую прокрутку тащит пользователь (handle + ось).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollTarget {
    /// Редактор панели: uniform_list (вертикальная). 8.1: полосы
    /// панелей независимы — индекс в ключе.
    Editor { pane: usize },
    /// Редактор панели: внешняя зона вокруг uniform_list
    /// (горизонтальная, editor_view.rs).
    EditorHorizontal { pane: usize },
    /// Левая панель (вертикальная).
    Panel,
    /// Оверлей выбора языка (вертикальная).
    Language,
    /// Таб-бар панели (горизонтальная).
    TabBar { pane: usize },
}

impl ScrollTarget {
    /// Ось цели: true — горизонтальная.
    pub(crate) fn horizontal(self) -> bool {
        matches!(
            self,
            ScrollTarget::EditorHorizontal { .. } | ScrollTarget::TabBar { .. }
        )
    }

    /// Уникальный ключ экземпляра вертикальной полосы (id элемента +
    /// debug_selector для GUI-тестов; глобальные ключи — разные
    /// экземпляры полос не путаются в debug_bounds).
    fn v_key(self) -> String {
        match self {
            ScrollTarget::Editor { pane } => format!("editor-vscroll-{pane}"),
            ScrollTarget::Panel => "panel-vscroll".to_string(),
            ScrollTarget::Language => "language-vscroll".to_string(),
            _ => "vscroll".to_string(),
        }
    }

    /// Ключ экземпляра горизонтальной полосы.
    fn h_key(self) -> String {
        match self {
            ScrollTarget::EditorHorizontal { pane } => format!("editor-hscroll-{pane}"),
            ScrollTarget::TabBar { pane } => format!("tabbar-hscroll-{pane}"),
            _ => "hscroll".to_string(),
        }
    }
}

/// Активный драг ползунка: координата мыши и прогресс скролла на
/// старте (двигаем ползунок на delta мыши, масштабируя её отношением
/// «весь скролл / ход ползунка»).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ThumbDrag {
    pub(crate) target: ScrollTarget,
    /// Координата мыши на старте (y вертикали / x горизонтали).
    pub(crate) start_mouse: f32,
    /// Прогресс скролла на старте (0..=max, положительный).
    pub(crate) start_scrolled: f32,
}

/// Геометрия ползунка (px, вдоль оси скролла).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollbarGeometry {
    /// Длина дорожки (размер контейнера вдоль оси).
    pub(crate) track_len: f32,
    /// Длина ползунка.
    pub(crate) thumb_len: f32,
    /// Смещение ползунка от начала дорожки.
    pub(crate) thumb_pos: f32,
    /// Максимальный прогресс скролла (>= 0; 0 — не скроллится).
    pub(crate) max_scroll: f32,
    /// Начало дорожки в координатах окна (для кликов по дорожке).
    pub(crate) track_origin: f32,
}

impl ScrollbarGeometry {
    /// Вертикальная геометрия из handle; `None` — контейнер не
    /// скроллится или ещё не отрисован (пустые bounds/max_offset).
    pub(crate) fn vertical(handle: &ScrollHandle) -> Option<Self> {
        let bounds = handle.bounds();
        let max = f32::from(handle.max_offset().height);
        if scroll_debug() {
            eprintln!(
                "[vscroll] bounds={}x{} max_h={max}",
                f32::from(bounds.size.width),
                f32::from(bounds.size.height)
            );
        }
        (max > 0.0 && f32::from(bounds.size.height) > 0.0).then(|| {
            Self::along_axis(
                f32::from(bounds.size.height),
                max,
                f32::from(handle.offset().y),
                f32::from(bounds.origin.y),
            )
        })
    }

    /// Горизонтальная геометрия из handle.
    pub(crate) fn horizontal(handle: &ScrollHandle) -> Option<Self> {
        let bounds = handle.bounds();
        let max = f32::from(handle.max_offset().width);
        if scroll_debug() {
            eprintln!(
                "[hscroll] bounds={}x{} max_w={max}",
                f32::from(bounds.size.width),
                f32::from(bounds.size.height)
            );
        }
        (max > 0.0 && f32::from(bounds.size.width) > 0.0).then(|| {
            Self::along_axis(
                f32::from(bounds.size.width),
                max,
                f32::from(handle.offset().x),
                f32::from(bounds.origin.x),
            )
        })
    }

    /// Общая арифметика обеих осей: `offset` — компонента вдоль оси
    /// (ОТРИЦАТЕЛЬНА при прокрутке вперёд, семантика gpui), длина
    /// контента = вьюпорт + запас прокрутки.
    fn along_axis(track_len: f32, max_scroll: f32, offset: f32, track_origin: f32) -> Self {
        let content_len = track_len + max_scroll;
        let thumb_len = (track_len * track_len / content_len).clamp(THUMB_MIN_LEN, track_len);
        // Овэрскролл-отскок (offset > 0) считается началом.
        let scrolled = (-offset).max(0.0);
        let travel = (track_len - thumb_len).max(1.0);
        let thumb_pos = (travel * scrolled / max_scroll).clamp(0.0, travel);
        ScrollbarGeometry { track_len, thumb_len, thumb_pos, max_scroll, track_origin }
    }

    /// Ход ползунка по дорожке (>= 1 — защита от деления на ноль).
    fn travel(&self) -> f32 {
        (self.track_len - self.thumb_len).max(1.0)
    }

    /// Прогресс скролла (0..=max) для клика по дорожке: ползунок
    /// центрируется на точку клика.
    fn scrolled_for_track_click(&self, mouse: f32) -> f32 {
        let wanted = (mouse - self.track_origin - self.thumb_len / 2.0).clamp(0.0, self.travel());
        wanted / self.travel() * self.max_scroll
    }
}

/// Диагностика полос (NOTEPAD_DEBUG_SCROLL=1): решение геометрии
/// каждой полосы в каждый кадр — для разбора «полоса пропала» вне
/// тестового стенда (паттерн NOTEPAD_DEBUG_MOUSE).
fn scroll_debug() -> bool {
    static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DEBUG.get_or_init(|| std::env::var("NOTEPAD_DEBUG_SCROLL").is_ok())
}

/// Текущий прогресс скролла handle вдоль оси (положительный).
fn scrolled_now(handle: &ScrollHandle, horizontal: bool) -> f32 {
    let offset = if horizontal {
        f32::from(handle.offset().x)
    } else {
        f32::from(handle.offset().y)
    };
    (-offset).max(0.0)
}

/// Применить прогресс скролла (положительный, клампится по max;
/// поперечная компонента offset сохраняется).
fn apply_scrolled(handle: &ScrollHandle, horizontal: bool, scrolled: f32) {
    let max = if horizontal {
        f32::from(handle.max_offset().width)
    } else {
        f32::from(handle.max_offset().height)
    };
    let scrolled = scrolled.clamp(0.0, max.max(0.0));
    let old = handle.offset();
    let new = if horizontal {
        point(px(-scrolled), old.y)
    } else {
        point(old.x, px(-scrolled))
    };
    handle.set_offset(new);
}

impl Workspace {
    /// ScrollHandle цели (редактор — base_handle uniform_list панели).
    pub(crate) fn scroll_handle_for(&self, target: ScrollTarget) -> ScrollHandle {
        match target {
            ScrollTarget::Editor { pane } => {
                self.panes[pane].scroll.0.borrow().base_handle.clone()
            }
            ScrollTarget::EditorHorizontal { pane } => self.panes[pane].h_scroll.clone(),
            ScrollTarget::Panel => self.panel_scroll.clone(),
            ScrollTarget::Language => self.language_scroll.clone(),
            ScrollTarget::TabBar { pane } => self.panes[pane].tab_scroll.clone(),
        }
    }

    /// Вертикальная полоса-ОВЕРЛЕЙ (панель/оверлей языка): дорожка
    /// (клик — прыжок) + ползунок (драг) поверх контента справа.
    pub(crate) fn render_vertical_scrollbar(
        &self,
        target: ScrollTarget,
        cx: &Context<Self>,
    ) -> AnyElement {
        self.vertical_scrollbar(target, cx, false)
    }

    /// Вертикальная полоса-ДОРОЖКА (редактор): СВОЯ полоса справа от
    /// контента (фон + линия у текста, место занимается ВСЕГДА —
    /// layout не прыгает при появлении прокрутки). Геометрия — та же
    /// (bounds скролл-контейнера; дорожка с ним в одной строке).
    pub(crate) fn render_vertical_scrollbar_lane(
        &self,
        target: ScrollTarget,
        cx: &Context<Self>,
    ) -> AnyElement {
        self.vertical_scrollbar(target, cx, true)
    }

    fn vertical_scrollbar(
        &self,
        target: ScrollTarget,
        cx: &Context<Self>,
        lane: bool,
    ) -> AnyElement {
        let g = ScrollbarGeometry::vertical(&self.scroll_handle_for(target));
        if g.is_none() && !lane {
            return div().into_any_element(); // оверлей без прокрутки — места не занимает
        }
        let key = target.v_key();
        let mut track = div().id(gpui::SharedString::from(format!("{key}-track")));
        track = if lane {
            track
                .w(px(TRACK_W))
                .h_full()
                .flex_shrink_0()
                .relative()
                .bg(rgb(crate::theme::current().scrollbar_track))
                .border_l_1()
                .border_color(rgb(crate::theme::current().scrollbar_track_border))
        } else {
            track.absolute().top_0().right_0().h_full().w(px(TRACK_W))
        }
        .debug_selector({
            let key = key.clone();
            move || format!("{key}-track")
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, e: &gpui::MouseDownEvent, _w, cx| {
                cx.stop_propagation();
                let handle = this.scroll_handle_for(target);
                let Some(g) = ScrollbarGeometry::vertical(&handle) else {
                    return;
                };
                apply_scrolled(&handle, false, g.scrolled_for_track_click(f32::from(e.position.y)));
                // Синхронная прокрутка панелей (8.1): клик по дорожке —
                // интерактивная прокрутка редактора.
                if let ScrollTarget::Editor { pane } = target {
                    this.sync_scroll_from(pane, cx);
                }
                cx.notify();
            }),
        );
        if let Some(g) = g {
            track = track.child(
                div()
                    .id(gpui::SharedString::from(format!("{key}-thumb")))
                    .debug_selector({
                        let key = key.clone();
                        move || format!("{key}-thumb")
                    })
                    .absolute()
                    .top(px(g.thumb_pos))
                    .left(px(2.0))
                    .w(px(TRACK_W - 4.0))
                    .h(px(g.thumb_len))
                    .rounded_sm()
                    .bg(rgb(crate::theme::current().scrollbar_thumb))
                    .hover(|d| d.bg(rgb(crate::theme::current().scrollbar_thumb_hover)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, e: &gpui::MouseDownEvent, _w, cx| {
                            cx.stop_propagation();
                            let handle = this.scroll_handle_for(target);
                            this.thumb_drag = Some(ThumbDrag {
                                target,
                                start_mouse: f32::from(e.position.y),
                                start_scrolled: scrolled_now(&handle, false),
                            });
                            cx.notify();
                        }),
                    ),
            );
        }
        track.into_any_element()
    }

    /// Горизонтальная полоса-ОВЕРЛЕЙ (таб-бар).
    pub(crate) fn render_horizontal_scrollbar(
        &self,
        target: ScrollTarget,
        cx: &Context<Self>,
    ) -> AnyElement {
        self.horizontal_scrollbar(target, cx, false)
    }

    /// Горизонтальная полоса-ДОРОЖКА (редактор): своя полоса ПОД
    /// контентом. Ход ползунка = ширина зоны скролла — правый хвост
    /// дорожки остаётся угловым квадратиком (классика), вертикальная
    /// полоса и ползунок не встречаются.
    pub(crate) fn render_horizontal_scrollbar_lane(
        &self,
        target: ScrollTarget,
        cx: &Context<Self>,
    ) -> AnyElement {
        self.horizontal_scrollbar(target, cx, true)
    }

    fn horizontal_scrollbar(
        &self,
        target: ScrollTarget,
        cx: &Context<Self>,
        lane: bool,
    ) -> AnyElement {
        let g = ScrollbarGeometry::horizontal(&self.scroll_handle_for(target));
        if g.is_none() && !lane {
            return div().into_any_element();
        }
        let key = target.h_key();
        let mut track = div().id(gpui::SharedString::from(format!("{key}-track")));
        track = if lane {
            track
                .h(px(TRACK_W))
                .w_full()
                .flex_shrink_0()
                .relative()
                .bg(rgb(crate::theme::current().scrollbar_track))
                .border_t_1()
                .border_color(rgb(crate::theme::current().scrollbar_track_border))
        } else {
            track.absolute().bottom_0().left_0().w_full().h(px(TRACK_W))
        }
        .debug_selector({
            let key = key.clone();
            move || format!("{key}-track")
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, e: &gpui::MouseDownEvent, _w, cx| {
                cx.stop_propagation();
                let handle = this.scroll_handle_for(target);
                let Some(g) = ScrollbarGeometry::horizontal(&handle) else {
                    return;
                };
                apply_scrolled(&handle, true, g.scrolled_for_track_click(f32::from(e.position.x)));
                cx.notify();
            }),
        );
        if let Some(g) = g {
            track = track.child(
                div()
                    .id(gpui::SharedString::from(format!("{key}-thumb")))
                    .debug_selector({
                        let key = key.clone();
                        move || format!("{key}-thumb")
                    })
                    .absolute()
                    .left(px(g.thumb_pos))
                    .top(px(2.0))
                    .h(px(TRACK_W - 4.0))
                    .w(px(g.thumb_len))
                    .rounded_sm()
                    .bg(rgb(crate::theme::current().scrollbar_thumb))
                    .hover(|d| d.bg(rgb(crate::theme::current().scrollbar_thumb_hover)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, e: &gpui::MouseDownEvent, _w, cx| {
                            cx.stop_propagation();
                            let handle = this.scroll_handle_for(target);
                            this.thumb_drag = Some(ThumbDrag {
                                target,
                                start_mouse: f32::from(e.position.x),
                                start_scrolled: scrolled_now(&handle, true),
                            });
                            cx.notify();
                        }),
                    ),
            );
        }
        track.into_any_element()
    }

    /// Двигать ползунок (mouse move активного драга). Вешается на
    /// контейнер зоны скролла; `true` — событие съедено (драг шёл).
    /// Move БЕЗ зажатой кнопки при живом драге — пропущенный up:
    /// завершаем драг (self-healing, см. модульный комментарий).
    pub(crate) fn on_scrollbar_drag_move(
        &mut self,
        event: &gpui::MouseMoveEvent,
        _window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(drag) = self.thumb_drag else {
            return false;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.thumb_drag = None;
            cx.notify();
            return true;
        }
        let horizontal = drag.target.horizontal();
        let handle = self.scroll_handle_for(drag.target);
        let geometry = if horizontal {
            ScrollbarGeometry::horizontal(&handle)
        } else {
            ScrollbarGeometry::vertical(&handle)
        };
        let Some(g) = geometry else {
            return true;
        };
        let mouse = if horizontal {
            f32::from(event.position.x)
        } else {
            f32::from(event.position.y)
        };
        let delta = (mouse - drag.start_mouse) * (g.max_scroll / g.travel());
        apply_scrolled(&handle, horizontal, drag.start_scrolled + delta);
        // Синхронная прокрутка панелей (8.1): драг ползунка
        // редакторной вертикали — интерактивная прокрутка.
        if let ScrollTarget::Editor { pane } = drag.target {
            self.sync_scroll_from(pane, cx);
        }
        cx.notify();
        true
    }

    /// Завершить драг (mouse up). Вешается на контейнер зоны скролла;
    /// `true` — событие съедено (драг был).
    pub(crate) fn on_scrollbar_drag_up(
        &mut self,
        _event: &gpui::MouseUpEvent,
        _window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.thumb_drag.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    //! Геометрия — чистая арифметика, без GUI (границы ползунка,
    //! масштаб драга, кламп клика по дорожке).

    use super::*;

    #[test]
    fn geometry_half_viewport_thumb() {
        // Контент 2×вьюпорта: ползунок — половина дорожки.
        let g = ScrollbarGeometry::along_axis(400.0, 400.0, 0.0, 10.0);
        assert!((g.thumb_len - 200.0).abs() < 0.01, "thumb {}", g.thumb_len);
        assert_eq!(g.thumb_pos, 0.0, "начало — ползунок вверху");
    }

    #[test]
    fn geometry_scrolled_moves_thumb() {
        let g = ScrollbarGeometry::along_axis(400.0, 1200.0, -600.0, 0.0);
        // thumb_len = 400*400/1600 = 100; ход 300; 600/1200 → середина.
        assert!((g.thumb_len - 100.0).abs() < 0.01);
        assert!((g.thumb_pos - 150.0).abs() < 0.01, "pos {}", g.thumb_pos);
    }

    #[test]
    fn geometry_min_thumb_and_clamps() {
        // Огромный контент: ползунок не короче минимума; прокрутка в
        // конец — ползунок у конца дорожки; овэрскролл — клампится.
        let g = ScrollbarGeometry::along_axis(400.0, 100_000.0, -100_000.0, 0.0);
        assert!((g.thumb_len - THUMB_MIN_LEN).abs() < 0.01);
        assert!((g.thumb_pos - (400.0 - THUMB_MIN_LEN)).abs() < 0.01);
        // 99% скролла — 99% хода (симметрия прогресса).
        let almost = ScrollbarGeometry::along_axis(400.0, 100_000.0, -99_000.0, 0.0);
        assert!((almost.thumb_pos - 0.99 * (400.0 - THUMB_MIN_LEN)).abs() < 0.01);
        let overscroll = ScrollbarGeometry::along_axis(400.0, 400.0, 50.0, 0.0);
        assert_eq!(overscroll.thumb_pos, 0.0, "положительный offset — начало");
    }

    #[test]
    fn track_click_centers_thumb() {
        let g = ScrollbarGeometry::along_axis(400.0, 1200.0, 0.0, 100.0);
        // Клик в середину дорожки (window y=300 = origin+200): центр
        // ползунка (len 100) хочет 150..250 → прогресс 150/300*1200.
        assert!((g.scrolled_for_track_click(300.0) - 600.0).abs() < 0.01);
        // Клики за краями клампятся в 0/max.
        assert_eq!(g.scrolled_for_track_click(0.0), 0.0);
        assert!((g.scrolled_for_track_click(1000.0) - 1200.0).abs() < 0.01);
    }
}
