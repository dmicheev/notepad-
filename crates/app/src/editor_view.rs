//! Рендер текста редактора (4.1, D9.1): разрез строк каретками и
//! выделениями (`split_line`/`line_carets` — чистые функции, есть
//! unit-тесты), virtualized-список видимых строк с гуттером
//! (закладки, номера, фолды) и подсветкой активной строки.
//!
//! Ловушка gpui (баг из ручной проверки 4.3): фон выделения вешается
//! на div СВОЕГО сегмента, а НЕ на контейнер строки — иначе частичное
//! выделение красит строку целиком (регрессионный GUI-тест
//! `selection_background_hugs_segment_not_line` в gui_tests.rs).

use gpui::{
    Context, MouseButton, Window, div, prelude::*, px, rgb, uniform_list,
};

use editor_core::{Document, Selection};

use crate::geometry::{FONT, FONT_SIZE, GUTTER_W, GUTTER_ZONE_W, LINE_HEIGHT};
use crate::theme::*;
use crate::workspace::Workspace;

/// Порог записи замера рендера видимых строк (бенчмарк 4.1, паттерн
/// spike): кадры дороже этого значения попадают в stderr.
const SLOW_FRAME_MS: f64 = 8.0;

/// Один кусок строки после разреза в позициях кареток/выделений:
/// `caret_before` — 2px-каретка перед куском, `selected` — фоновая
/// подсветка. Позиции без метрик шрифта (D8.1/D9.1).
pub(crate) struct LineSegment {
    pub(crate) text: String,
    pub(crate) selected: bool,
    pub(crate) caret_before: bool,
}

/// Разрезать текст строки в границах кареток (char-колонки, «каретка
/// перед колонкой») и выделений; соседние куски склеиваются по границам.
/// Обобщает рендер каретки D8.1 на несколько кареток в строке (4.1→4.3).
pub(crate) fn split_line(content: &str, carets: &[usize], sel_ranges: &[(usize, usize)]) -> Vec<LineSegment> {
    let len = content.chars().count();
    let clamp = |c: usize| c.min(len);
    let mut bounds: Vec<usize> = Vec::with_capacity(carets.len() * 2 + sel_ranges.len() * 2 + 2);
    bounds.push(0);
    bounds.push(len);
    bounds.extend(carets.iter().map(|c| clamp(*c)));
    for (s, e) in sel_ranges {
        bounds.push(clamp(*s));
        bounds.push(clamp(*e));
    }
    bounds.sort_unstable();
    bounds.dedup();

    let mut segments = Vec::with_capacity(bounds.len());
    let byte_at = |col: usize| {
        content
            .char_indices()
            .nth(col)
            .map(|(b, _)| b)
            .unwrap_or(content.len())
    };
    for (i, &b) in bounds.iter().enumerate() {
        let caret = carets.iter().any(|c| clamp(*c) == b);
        if i + 1 < bounds.len() {
            let next = bounds[i + 1];
            if next > b {
                let text = content[byte_at(b)..byte_at(next)].to_string();
                let selected = sel_ranges.iter().any(|(s, e)| b >= clamp(*s) && next <= clamp(*e));
                segments.push(LineSegment { text, selected, caret_before: caret });
                continue;
            }
        }
        if caret {
            segments.push(LineSegment {
                text: String::new(),
                selected: false,
                caret_before: true,
            });
        }
    }
    segments
}

/// Каретки и выделения одной строки для рендера: char-колонки кареток
/// и пересечения выделений со строкой (без терминатора). Источник —
/// список курсоров вкладки (в 4.1 — единственный `tab.cursor`).
pub(crate) fn line_carets(
    cursors: &[Selection],
    doc: &Document,
    line: usize,
    content_len: usize,
) -> (Vec<usize>, Vec<(usize, usize)>) {
    let ls = doc.line_to_char(line);
    let le = ls + content_len; // конец содержимого строки (терминатор не входит)
    let mut carets = Vec::new();
    let mut ranges = Vec::new();
    for sel in cursors {
        if sel.is_caret() {
            if ls <= sel.head() && sel.head() <= le {
                carets.push(sel.head() - ls);
            }
        } else {
            let a = sel.start.max(ls);
            let b = sel.end.min(le);
            if a < b {
                ranges.push((a - ls, b - ls));
            }
        }
    }
    (carets, ranges)
}

impl Workspace {
    pub(crate) fn render_editor(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.model.is_empty() {
            return div()
                .id("editor-empty")
                .flex_1()
                .min_h_0()
                .track_focus(&self.focus)
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(DIM))
                .child("No tabs open — press Cmd+O to open a file, or + for a new tab")
                .into_any_element();
        }

        let tab = self.model.active().expect("checked non-empty");

        // uniform_list рендерит ВИДИМЫЕ строки (мапа folding, D9.5);
        // ensure_fold_view вызван в Render::render.
        let line_count = self.visible_len();
        // Подсветка активной строки — по ГЛАВНОЙ каретке и только когда
        // она — чистая каретка, без выделения (D9.1).
        let active_line = tab.primary().is_caret().then(|| tab.cursor_line());
        // Все курсоры вкладки (4.3).
        let cursors = tab.cursors.clone();

        div()
            .id("editor")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::handle_key))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_editor_mouse_down))
            .on_mouse_move(cx.listener(Self::on_editor_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_editor_mouse_up))
            .child(
                uniform_list(
                    "lines",
                    line_count,
                    cx.processor(
                        move |this: &mut Self,
                              range: std::ops::Range<usize>,
                              _window: &mut Window,
                              cx: &mut Context<Self>| {
                            // Бенчмарк 4.1 (паттерн spike): время сборки
                            // видимых строк; кадры дороже порога — в
                            // stderr, остальные не шумят.
                            let started = std::time::Instant::now();
                            let Some(tab) = this.model.active() else {
                                return Vec::new();
                            };
                            let mut items = Vec::with_capacity(range.len());
                            for ix in range.clone() {
                                // ix — индекс ВИДИМОЙ строки; real — строка документа.
                                let real = this.visible_to_real(ix);
                                let content = tab.doc.line_content(real);
                                let content_len = content.chars().count();
                                let (carets, sel_ranges) =
                                    line_carets(&cursors, &tab.doc, real, content_len);
                                let is_active_line = active_line == Some(real);
                                let bookmarked = tab.doc.line_meta().is_bookmarked(real);
                                // Маркер свёртывания: ▾ (развернут, есть тело),
                                // ▸ (свёрнут), пусто (не заголовок).
                                let foldable = crate::folding::is_foldable(&tab.doc, real);
                                let folded = foldable && tab.doc.line_meta().is_folded(real);
                                let fold_marker = if folded {
                                    "▸"
                                } else if foldable {
                                    "▾"
                                } else {
                                    ""
                                };
                                // Текстовый контейнер: сегменты строки с
                                // каретками (2px-див) и подсветкой выделений.
                                let mut text_cell = div()
                                    .flex()
                                    .items_center()
                                    .text_size(px(FONT_SIZE))
                                    .font_family(FONT)
                                    .whitespace_nowrap();
                                for (si, seg) in split_line(&content, &carets, &sel_ranges)
                                    .into_iter()
                                    .enumerate()
                                {
                                    text_cell = text_cell.when(seg.caret_before, |d| {
                                        d.child(
                                            div()
                                                .w(px(2.0))
                                                .h(px(LINE_HEIGHT - 4.0))
                                                .bg(rgb(CARET))
                                                .flex_shrink_0(),
                                        )
                                    });
                                    if !seg.text.is_empty() {
                                        // Фон выделения — на div САМОГО сегмента
                                        // (не контейнера строки!), иначе синим
                                        // красится вся строка при любом частичном
                                        // выделении (баг из ручной проверки 4.3).
                                        let mut seg_div = div().child(seg.text);
                                        if seg.selected {
                                            seg_div = seg_div.bg(rgb(SELECTION));
                                        }
                                        // Границы сегмента для GUI-тестов рендера
                                        // (no-op без test-support).
                                        let key = format!("seg:{real}:{si}");
                                        seg_div = seg_div.debug_selector(move || key.clone());
                                        text_cell = text_cell.child(seg_div);
                                    }
                                }
                                items.push(
                                    div()
                                        .h(px(LINE_HEIGHT))
                                        .flex()
                                        .w_full()
                                        .when(is_active_line, |d| d.bg(rgb(CARET_LINE)))
                                        // Гуттер: [fold-зона | номер | закладка].
                                        .child(
                                            div()
                                                .w(px(GUTTER_W))
                                                .flex_shrink_0()
                                                .flex()
                                                .items_center()
                                                // Зона закладки (клик — toggle).
                                                .child(
                                                    div()
                                                        .id(("bm", real))
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(move |this, _e, _w, cx| {
                                                                cx.stop_propagation();
                                                                this.toggle_bookmark_at(real, cx);
                                                            }),
                                                        )
                                                        .w(px(GUTTER_ZONE_W))
                                                        .h_full()
                                                        .flex_shrink_0()
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .text_size(px(9.0))
                                                        .text_color(rgb(ACCENT))
                                                        .child(if bookmarked { "●" } else { "" }),
                                                )
                                                // Номер строки (подсветка активной).
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .text_size(px(11.0))
                                                        .text_color(rgb(GUTTER))
                                                        .text_right()
                                                        .child(format!("{}", real + 1)),
                                                )
                                                // Зона свёртывания: клик — toggle фолда.
                                                .child({
                                                    let zone = div()
                                                        .id(("fold", real))
                                                        .w(px(GUTTER_ZONE_W))
                                                        .h_full()
                                                        .flex_shrink_0()
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .text_size(px(9.0))
                                                        .text_color(rgb(GUTTER))
                                                        .child(fold_marker);
                                                    if foldable {
                                                        zone.on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(move |this, _e, _w, cx| {
                                                                cx.stop_propagation();
                                                                this.toggle_fold_at(real, cx);
                                                            }),
                                                        )
                                                        .hover(|d| d.text_color(rgb(TEXT)))
                                                    } else {
                                                        zone
                                                    }
                                                }),
                                        )
                                        .child(text_cell),
                                );
                            }
                            let frame_ms = started.elapsed().as_secs_f64() * 1000.0;
                            if frame_ms > SLOW_FRAME_MS {
                                eprintln!(
                                    "[render] visible {}..{} built in {:.1} ms",
                                    range.start, range.end, frame_ms
                                );
                            }
                            items
                        },
                    ),
                )
                .track_scroll(self.scroll.clone())
                .h_full(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod render_tests {
    /// Unit-тесты обобщённого рендера строки (4.1): разрез текста
    /// каретками и выделениями — чистая функция без GUI.
    use super::split_line;

    fn texts(segs: &[super::LineSegment]) -> Vec<String> {
        segs.iter().map(|s| s.text.clone()).collect()
    }

    #[test]
    fn no_carets_single_segment() {
        let segs = split_line("hello", &[], &[]);
        assert_eq!(texts(&segs), vec!["hello"]);
        assert!(!segs[0].selected && !segs[0].caret_before);
    }

    #[test]
    fn single_caret_splits_in_two() {
        // Каретка перед колонкой 2: "he" | ▮ | "llo" (каретка рендерится
        // перед куском, идущим ПОСЛЕ позиции).
        let segs = split_line("hello", &[2], &[]);
        assert_eq!(texts(&segs), vec!["he", "llo"]);
        assert!(!segs[0].caret_before);
        assert!(segs[1].caret_before);
    }

    #[test]
    fn three_carets_on_one_line() {
        // Мульти-курсор (4.3): каретки в 0 и в конце строки. Границы
        // дедуплицируются; каретка в конце — отдельный пустой сегмент.
        let segs = split_line("abc", &[0, 3], &[]);
        assert_eq!(texts(&segs), vec!["abc", ""]);
        assert_eq!(segs.iter().filter(|s| s.caret_before).count(), 2);
    }

    #[test]
    fn selection_highlight_middle() {
        let segs = split_line("hello world", &[], &[(6, 11)]);
        assert_eq!(texts(&segs), vec!["hello ", "world"]);
        assert!(!segs[0].selected);
        assert!(segs[1].selected);
    }

    #[test]
    fn caret_inside_selection() {
        // Выделение [1..4) и каретка в 2: каретка рендерится внутри
        // подсвеченного куска.
        let segs = split_line("abcdef", &[2], &[(1, 4)]);
        assert_eq!(texts(&segs), vec!["a", "b", "cd", "ef"]);
        assert!(segs[1].selected);
        assert!(segs[2].selected && segs[2].caret_before);
        assert!(!segs[0].selected && !segs[3].selected);
    }

    #[test]
    fn caret_at_line_end_and_start() {
        // Каретка в 0 и в конце (== len): текст + два пустых сегмента
        // с каретками до/после всего текста.
        let segs = split_line("ab", &[0, 2], &[]);
        assert_eq!(texts(&segs), vec!["ab", ""]);
        assert_eq!(segs.iter().filter(|s| s.caret_before).count(), 2);
    }

    #[test]
    fn out_of_range_positions_clamped() {
        let segs = split_line("ab", &[9], &[(1, 99)]);
        assert_eq!(texts(&segs), vec!["a", "b", ""]);
        assert!(segs[1].selected);
        assert_eq!(segs.iter().filter(|s| s.caret_before).count(), 1);
    }
}
