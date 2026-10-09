//! Рендер текста редактора (4.1, D9.1 + 8.1/D15.1): разрез строк
//! каретками и выделениями (`split_line`/`line_carets` — чистые
//! функции, есть unit-тесты), virtualized-список видимых строк с
//! гуттером (закладки, номера, фолды) и подсветкой активной строки.
//!
//! Split view: `render_editor(pi)` рендерит панель `pi` ЕЁ вкладкой и
//! scroll-handle'ами; слушатели несут индекс панели (мышь/колесо/
//! клавиши — editor_mouse/editor_actions). Панель поиска рисуется
//! только в активной панели (File-scope поиск привязан к её вкладке).
//!
//! Ловушка gpui (баг из ручной проверки 4.3): фон выделения вешается
//! на div СВОЕГО сегмента, а НЕ на контейнер строки — иначе частичное
//! выделение красит строку целиком (регрессионный GUI-тест
//! `selection_background_hugs_segment_not_line` в gui_tests.rs).
//!
//! Липкая шапка CSV (D12.5): когда строка 0 целиком ушла из вида,
//! над списком рендерится ABSOLUTE-копия заголовка (вне uniform_list)
//! с цветами колонок и нижней рамкой; клик — прокрутка к началу.
//! На месте (начало файла) строка 0 выделяется фоном + АБСОЛЮТНЫМ
//! 1px-дивом-разделителем — border на строке списка ломает разметку
//! uniform_list (строка обязана быть ровно LINE_HEIGHT).

use gpui::{
    Context, MouseButton, Window, div, point, prelude::*, px, rgb, uniform_list,
};

use editor_core::highlight::Span;
use editor_core::{Document, Selection};

use crate::geometry::{GUTTER_W, GUTTER_ZONE_W, sticky_header_visible};
use crate::workspace::Workspace;

/// Порог записи замера рендера видимых строк (бенчмарк 4.1, паттерн
/// spike): кадры дороже этого значения попадают в stderr.
const SLOW_FRAME_MS: f64 = 8.0;

/// Хвост ширины горизонтального скролла (px): запас за концом самой
/// длинной строки — табы моно-шрифта шире advance, курсор за концом
/// строки должен быть виден.
const H_SCROLL_TAIL: f32 = 40.0;

/// Один кусок строки после разреза в позициях кареток/выделений:
/// `caret_before` — 2px-каретка перед куском, `selected` — фоновая
/// подсветка, `class` — класс стиля подсветки (5.2; None — обычный
/// текст, цвет наследуется), `search_hit`/`search_current` — совпадение
/// поиска/текущее совпадение (6.1; приоритет фона: выделение &gt;
/// текущее &gt; совпадение). Позиции без метрик шрифта (D8.1/D9.1).
pub(crate) struct LineSegment {
    pub(crate) text: String,
    pub(crate) selected: bool,
    pub(crate) caret_before: bool,
    pub(crate) class: Option<u8>,
    pub(crate) search_hit: bool,
    pub(crate) search_current: bool,
}

/// Разрезать текст строки в границах кареток (char-колонки, «каретка
/// перед колонкой») и выделений; соседние куски склеиваются по границам.
/// Обобщает рендер каретки D8.1 на несколько кареток в строке (4.1→4.3).
#[allow(dead_code)] // API-совместимость 4.x: рендер зовёт split_line_styled; тесты — split_line (round-trip)
pub(crate) fn split_line(
    content: &str,
    carets: &[usize],
    sel_ranges: &[(usize, usize)],
) -> Vec<LineSegment> {
    split_line_full(content, carets, sel_ranges, &[], &[])
}

/// `split_line` + цветовые диапазоны подсветки (5.2): сегменты режутся
/// ТАКЖЕ по границам цветов (диапазоны в char-координатах строки,
/// без терминатора); цвет применяется ВНУТРИ сегмента каретки/выделения
/// (фон выделения и каретка сохраняются). Пустые `colors` → вывод
/// бит-в-бит как `split_line` (round-trip без подсветки).
pub(crate) fn split_line_styled(
    content: &str,
    carets: &[usize],
    sel_ranges: &[(usize, usize)],
    colors: &[Span],
) -> Vec<LineSegment> {
    split_line_full(content, carets, sel_ranges, colors, &[])
}

/// Полный разрез: каретки + выделения + цвета + совпадения поиска
/// (6.1). `hits` — col-диапазоны совпадений, `current_flags` — какой
/// из них текущий (длины синхронны hits).
pub(crate) fn split_line_full(
    content: &str,
    carets: &[usize],
    sel_ranges: &[(usize, usize)],
    colors: &[Span],
    hits: &[((usize, usize), bool)],
) -> Vec<LineSegment> {
    let len = content.chars().count();
    let clamp = |c: usize| c.min(len);
    let mut bounds: Vec<usize> =
        Vec::with_capacity(carets.len() * 2 + sel_ranges.len() * 2 + colors.len() * 2 + hits.len() * 4 + 2);
    bounds.push(0);
    bounds.push(len);
    bounds.extend(carets.iter().map(|c| clamp(*c)));
    for (s, e) in sel_ranges {
        bounds.push(clamp(*s));
        bounds.push(clamp(*e));
    }
    for sp in colors {
        if sp.end > sp.start {
            bounds.push(clamp(sp.start as usize));
            bounds.push(clamp(sp.end as usize));
        }
    }
    for ((s, e), _) in hits {
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
                let selected =
                    sel_ranges.iter().any(|(s, e)| b >= clamp(*s) && next <= clamp(*e));
                let class = colors
                    .iter()
                    .find(|sp| b >= clamp(sp.start as usize) && next <= clamp(sp.end as usize))
                    .map(|sp| sp.class);
                // Текущее совпадение имеет приоритет над обычным
                // (сегмент может лежать в обоих).
                let mut search_current = false;
                let mut search_hit = false;
                for ((s, e), cur) in hits {
                    if b >= clamp(*s) && next <= clamp(*e) {
                        if *cur {
                            search_current = true;
                        } else {
                            search_hit = true;
                        }
                    }
                }
                segments.push(LineSegment {
                    text,
                    selected,
                    caret_before: caret,
                    class,
                    search_hit,
                    search_current,
                });
                continue;
            }
        }
        if caret {
            segments.push(LineSegment {
                text: String::new(),
                selected: false,
                caret_before: true,
                class: None,
                search_hit: false,
                search_current: false,
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
    pub(crate) fn render_editor(
        &mut self,
        pi: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let focus = self.panes[pi].focus.clone();
        if self.panes[pi].model.is_empty() {
            return div()
                .id(("editor-empty", pi))
                .flex_1()
                .min_h_0()
                .track_focus(&focus)
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(crate::theme::current().dim))
                .child(crate::i18n::t(cx, "editor.empty_hint"))
                .into_any_element();
        }

        // Липкая шапка CSV (D12.5): предикат по scroll-offset (offset.y
        // отрицателен при прокрутке вниз — семантика D9.2) ПАНЕЛИ.
        // Спаны строки 0 прогреваем ЗДЕСЬ, до чтения: ниже вкладка
        // берётся только на чтение. CSV stateless — ensure_window(0..1)
        // дёшев; при порог-отключённой подсветке движок без синтаксиса —
        // no-op, шапка липнет без цветов (решение D12.5).
        let is_csv = self.panes[pi].model.active().is_some_and(|t| t.is_csv());
        let sticky = is_csv
            && sticky_header_visible(
                f32::from(self.panes[pi].scroll.0.borrow().base_handle.offset().y),
            );
        let header = if sticky {
            if let Some(tab) = self.panes[pi].model.active_mut() {
                tab.highlight.ensure_window(&tab.doc, 0, 1);
            }
            self.panes[pi].model.active().map(|tab| {
                // Arc-клон спанов строки 0 (нет спанов — пусто: без цвета).
                let colors = tab.highlight.spans(0).cloned().unwrap_or_default();
                (tab.doc.line_content(0), colors)
            })
        } else {
            None
        };

        // uniform_list рендерит ВИДИМЫЕ строки панели (мапа folding,
        // D9.5); ensure_fold_view вызван в Render::render.
        let line_count = self.visible_len_of(pi);
        // Подсветка активной строки — по ГЛАВНОЙ каретке и только когда
        // она — чистая каретка, без выделения (D9.1).
        let active_line = self.panes[pi]
            .model
            .active()
            .and_then(|t| t.primary().is_caret().then(|| t.cursor_line()));
        // Все курсоры вкладки панели (4.3).
        let cursors = self.panes[pi]
            .model
            .active()
            .map(|t| t.cursors.clone())
            .unwrap_or_default();
        // Ширина контента горизонтального скролла: гуттер + самая
        // широкая виденная строка (ракетка рендера, tab_model) +
        // хвост; advance моноширинного Menlo — колоночная арифметика
        // точна (табы чуть шире — хвост вбирает). Меньше вьюпорта —
        // не бывает: список ещё и min_w_full. Считается ДО заёма
        // вкладки ниже (char_advance — &mut self).
        let max_cols = self.panes[pi]
            .model
            .active()
            .map(|t| t.max_seen_cols())
            .unwrap_or(0);
        let advance = self.char_advance(window);
        let content_w = GUTTER_W + max_cols as f32 * advance + H_SCROLL_TAIL;
        let pane_scroll = self.panes[pi].scroll.clone();
        let h_scroll = self.panes[pi].h_scroll.clone();
        let search_here = pi == self.active_pane;
        // Шрифт редактора (8.3): применяется к строкам немедленно.
        let font_family = self.font.family.clone();
        let font_size = self.font.size;
        let line_h = self.font.line_height();

        let mut editor = div()
            .id(("editor", pi))
            // relative — якорь absolute-оверлея липкой шапки (D12.5)
            // и панели поиска. flex-колонка: [строка: зона скролла |
            // вертикальная дорожка] + горизонтальная дорожка снизу —
            // ползунки в СВОИХ полосах, текст не перекрывается.
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .track_focus(&focus)
            .on_key_down(cx.listener(move |this, e, window, cx| {
                this.handle_key_pane(pi, e, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e, window, cx| {
                    this.on_editor_mouse_down_pane(pi, e, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e, window, cx| {
                    this.on_editor_right_click_pane(pi, e, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(move |this, e, window, cx| {
                this.on_editor_mouse_move_pane(pi, e, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, e, window, cx| {
                    this.on_editor_mouse_up_pane(pi, e, window, cx);
                }),
            )
            .child(
                div()
                    .id(("editor-content-row", pi))
                    .flex()
                    .flex_1()
                    .min_h_0()
                    // Зона горизонтального скролла (внешняя, вокруг
                    // списка): вертикаль остаётся в uniform_list — оси
                    // независимы. min_w_0 — делит строку с дорожкой.
                    .child(
                        div()
                            .id(("editor-hscroll", pi))
                            .flex_1()
                            .min_w_0()
                            .relative()
                            .overflow_x_scroll()
                            .track_scroll(&h_scroll)
                            .child(
                uniform_list(
                    ("lines", pi),
                    line_count,
                    cx.processor(
                        move |this: &mut Self,
                              range: std::ops::Range<usize>,
                              window: &mut Window,
                              cx: &mut Context<Self>| {
                            // Бенчмарк 4.1 (паттерн spike): время сборки
                            // видимых строк; кадры дороже порога — в
                            // stderr, остальные не шумят.
                            let started = std::time::Instant::now();
                            // real-строки окна считаем ДО мутабельного
                            // заёма вкладки (движок подсветки — &mut).
                            let reals: Vec<usize> =
                                range.clone().map(|ix| this.visible_to_real_of(pi, ix)).collect();
                            // Совпадения поиска (6.1) для окна — только
                            // в активной панели (File-scope поиск
                            // привязан к её вкладке); тоже ДО заёма.
                            let line_hits: Vec<Vec<((usize, usize), bool)>> = if search_here {
                                reals
                                    .iter()
                                    .copied()
                                    .map(|real| {
                                        let (ranges, curs) = this.search_line_hits(real);
                                        ranges.into_iter().zip(curs).collect()
                                    })
                                    .collect()
                            } else {
                                vec![Vec::new(); reals.len()]
                            };
                            let Some(pane) = this.panes.get_mut(pi) else {
                                return Vec::new();
                            };
                            let Some(tab) = pane.model.active_mut() else {
                                return Vec::new();
                            };
                            // Догон подсветки не завершён (бюджет
                            // 2 мс/кадр, ensure_window == false) —
                            // просим следующий кадр: прогрессивная
                            // подсветка добирает окно сама. Без этого
                            // после РЕЗКОГО прыжка (трекпад-флик)
                            // текст оставался без цвета до первого
                            // события мыши — плавный скролл кадры
                            // даёт, прыжок один (баг ручной проверки).
                            let mut highlight_pending = false;
                            // Подсветка (5.2): прогрев окна видимых строк
                            // (догон с бюджетом внутри движка); движок
                            // живёт во вкладке — НЕ аллоцируется на кадр.
                            if !reals.is_empty() {
                                let first = *reals.first().unwrap();
                                let count = reals.last().unwrap() + 1 - first;
                                highlight_pending =
                                    !tab.highlight.ensure_window(&tab.doc, first, count);
                            }
                            let mut items = Vec::with_capacity(range.len());
                            // Ракетка ширины контента (горизонтальный
                            // скролл): шире виденного — запомнить и
                            // перерисовать зону с новой шириной.
                            let mut width_grew = false;
                            // reals/line_hits уже вычислены (заём this.model — мут).
                            for (real, hits) in reals.iter().copied().zip(line_hits) {
                                let content = tab.doc.line_content(real);
                                let content_len = content.chars().count();
                                width_grew |= tab.note_line_width(content_len);
                                let (carets, sel_ranges) =
                                    line_carets(&cursors, &tab.doc, real, content_len);
                                // Цветовые диапазоны строки (кэш движка;
                                // None — строка ещё не догнана/без языка).
                                let colors: &[Span] = tab
                                    .highlight
                                    .spans(real)
                                    .map(|spans| spans.as_slice())
                                    .unwrap_or(&[]);
                                let is_active_line = active_line == Some(real);
                                // hits — совпадения поиска строки (подготов
                                // лены выше, до заёма вкладки).
                                // Шапка CSV на месте (D12.5): real-строка 0
                                // выделяется фоном; РАМКУ несёт только
                                // липкий оверлей — здесь АБСОЛЮТНЫЙ 1px-див
                                // (absolute не влияет на layout, высота
                                // строки ровно LINE_HEIGHT — ловушка
                                // uniform_list). Фон шапки сильнее подсветки
                                // активной строки (bg применяется позже).
                                let header_inplace = is_csv && real == 0;
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
                                    .text_size(px(font_size))
                                    .font_family(font_family.clone())
                                    .whitespace_nowrap();
                                for (si, seg) in split_line_full(
                                    &content,
                                    &carets,
                                    &sel_ranges,
                                    colors,
                                    &hits,
                                )
                                .into_iter()
                                .enumerate()
                                {
                                    text_cell = text_cell.when(seg.caret_before, |d| {
                                        d.child(
                                            div()
                                                .w(px(2.0))
                                                .h(px(line_h - 4.0))
                                                .bg(rgb(crate::theme::current().caret))
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
                                            seg_div = seg_div.bg(rgb(crate::theme::current().selection));
                                        } else if seg.search_current {
                                            // Приоритет фона: выделение &gt;
                                            // текущее совпадение &gt; совпадение
                                            // (D13); активная строка — фон row,
                                            // сегменты поверх неё.
                                            seg_div = seg_div.bg(rgb(crate::theme::current().search_current));
                                        } else if seg.search_hit {
                                            seg_div = seg_div.bg(rgb(crate::theme::current().search_match));
                                        }
                                        // Цвет подсветки — тоже на сегменте;
                                        // класс 0/None не задаёт цвет —
                                        // наследуется TEXT (round-trip D12.1).
                                        if let Some(class) = seg.class {
                                            seg_div = seg_div.text_color(rgb(crate::theme::current().syntax_color(class)));
                                        }
                                        // Границы сегмента для GUI-тестов рендера
                                        // (no-op без test-support).
                                        let key = format!("seg:{real}:{si}");
                                        seg_div = seg_div.debug_selector(move || key.clone());
                                        text_cell = text_cell.child(seg_div);
                                    }
                                }
                                let mut row = div()
                                    .h(px(line_h))
                                    .flex()
                                    .w_full()
                                    .when(is_active_line, |d| d.bg(rgb(crate::theme::current().caret_line)))
                                    .when(header_inplace, |d| {
                                        // relative — якорь 1px-разделителя;
                                        // маркер для GUI-тестов «на месте».
                                        d.relative()
                                            .bg(rgb(crate::theme::current().csv_header_bg))
                                            .debug_selector(|| "csv-header-inplace".to_string())
                                    })
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
                                                        .id(("bm", pi * 1_000_000_000 + real))
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(move |this, _e, _w, cx| {
                                                                cx.stop_propagation();
                                                                this.activate_pane(pi, cx);
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
                                                        .text_color(rgb(crate::theme::current().accent))
                                                        .child(if bookmarked { "●" } else { "" }),
                                                )
                                                // Номер строки (подсветка активной).
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .text_size(px(11.0))
                                                        .text_color(rgb(crate::theme::current().gutter))
                                                        .text_right()
                                                        .child(format!("{}", real + 1)),
                                                )
                                                // Зона свёртывания: клик — toggle фолда.
                                                .child({
                                                    let zone = div()
                                                        .id(("fold", pi * 1_000_000_000 + real))
                                                        .w(px(GUTTER_ZONE_W))
                                                        .h_full()
                                                        .flex_shrink_0()
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .text_size(px(9.0))
                                                        .text_color(rgb(crate::theme::current().gutter))
                                                        .child(fold_marker);
                                                    if foldable {
                                                        zone.on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(move |this, _e, _w, cx| {
                                                                cx.stop_propagation();
                                                                this.activate_pane(pi, cx);
                                                                this.toggle_fold_at(real, cx);
                                                            }),
                                                        )
                                                        .hover(|d| d.text_color(rgb(crate::theme::current().text)))
                                                    } else {
                                                        zone
                                                    }
                                                 }),
                                          )
                                          .child(text_cell);
                                // Шапка на месте: абсолютный 1px-разделитель
                                // снизу (absolute не влияет на layout — высота
                                // строки остаётся ровно LINE_HEIGHT).
                                if header_inplace {
                                    row = row.child(
                                        div()
                                            .absolute()
                                            .bottom_0()
                                            .left_0()
                                            .w_full()
                                            .h(px(1.0))
                                            .bg(rgb(crate::theme::current().csv_header_border)),
                                    );
                                }
                                items.push(row);
                            }
                            let frame_ms = started.elapsed().as_secs_f64() * 1000.0;
                            if frame_ms > SLOW_FRAME_MS {
                                eprintln!(
                                    "[render] visible {}..{} built in {:.1} ms",
                                    range.start, range.end, frame_ms
                                );
                            }
                            // Бенчмарк-стенд (D11.1): сбор длительностей
                            // кадров по фазам (включён — только при
                            // NOTEPAD_BENCH=1).
                            crate::bench::record_frame(frame_ms);
                            // Ширина контента выросла ИЛИ подсветка не
                            // догнала окно — нужен следующий кадр.
                            // ЛОВУШКА gpui 0.2.2: cx.notify() ИЗ paint
                            // НЕ ставит window-dirty (invalidate_view
                            // при draw_phase != None только пишет
                            // dirty_views, которые читает СЛЕДУЮЩИЙ
                            // draw — а его без dirty не будет; notify
                            // терялся и подсветка после прыжка гасла).
                            // request_animation_frame переносит notify
                            // на тик frame-callback'а — он исполняется
                            // ДО проверки is_dirty() и честно ставит
                            // dirty: самозапуск кадра работает.
                            if width_grew || highlight_pending {
                                window.request_animation_frame();
                            }
                            items
                        },
                    ),
                )
                .track_scroll(pane_scroll)
                // Фиксированная ширина контента (горизонтальный
                // скролл зоны): строки w_full заполняют её целиком —
                // фоны активной строки/шапки тянутся на всю ширину
                // прокрутки; min_w_full — короче вьюпорта не бывает.
                .w(px(content_w))
                .min_w_full()
                .h_full(),
                    )
                    // Разделение осей колеса (ловушка gpui 0.2.2, баг
                    // «вертикальное колесо крутит нижнюю полосу»):
                    // скролл-контейнер с ОДНОЙ осью конвертирует колесо
                    // другой в свою (div.rs: delta_x = delta.y при
                    // overflow.y != Scroll; флаг restrict_scroll_to_axis
                    // без сеттера). Порядок диспетчеризации: слушатели
                    // регистрируются при paint РОДИТЕЛЯ ДО детей, bubble
                    // идёт по регистрации В ОБРАТНОМ порядке — значит
                    // ПОСЛЕДНИЙ ребёнок зоны срабатывает РАНЬШЕ
                    // внутренних слушателей списка и зоны. Этот
                    // прозрачный оверлей сам раскладывает дельту по
                    // осям (editor_mouse::on_editor_wheel) и гасит
                    // событие — конвертации не происходит вовсе.
                    // Хитбокс Normal: клики по строкам/гуттеру
                    // проходят сквозь.
                    .child(
                        div()
                            .id(("editor-wheel-guard", pi))
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .on_scroll_wheel(cx.listener(move |this, e, window, cx| {
                                this.on_editor_wheel_pane(pi, e, window, cx);
                            })),
                    ),
            )
            // Вертикальная дорожка — сосед зоны (своя полоса, не
            // поверх текста); wheel над ней тоже через on_editor_wheel.
            .child(
                div()
                    .id(("editor-vlane", pi))
                    .h_full()
                    .on_scroll_wheel(cx.listener(move |this, e, window, cx| {
                        this.on_editor_wheel_pane(pi, e, window, cx);
                    }))
                    .child(
                        self.render_vertical_scrollbar_lane(
                            crate::scrollbar::ScrollTarget::Editor { pane: pi },
                            cx,
                        ),
                    ),
            ),
            )
            // Горизонтальная дорожка — под строкой контента; правый
            // хвост — угловой квадратик (ход ползунка = ширина зоны).
            .child(
                div()
                    .id(("editor-hlane", pi))
                    .on_scroll_wheel(cx.listener(move |this, e, window, cx| {
                        this.on_editor_wheel_pane(pi, e, window, cx);
                    }))
                    .child(
                        self.render_horizontal_scrollbar_lane(
                            crate::scrollbar::ScrollTarget::EditorHorizontal { pane: pi },
                            cx,
                        ),
                    ),
            );
        // Полосы прокрутки (scrollbar.rs) — оверлеи краёв редактора:
        // вертикальная после uniform_list, горизонтальная снизу;
        // липкая шапка/панель поиска выше по z-order (паттерн
        // детей-оверлеев).
        editor = editor.child(
            self.render_vertical_scrollbar(crate::scrollbar::ScrollTarget::Editor { pane: pi }, cx),
        );
        editor = editor.child(self.render_horizontal_scrollbar(
            crate::scrollbar::ScrollTarget::EditorHorizontal { pane: pi },
            cx,
        ));
        // Липкая шапка (D12.5) — ребёнок контейнера редактора, добавлен
        // ПОСЛЕ uniform_list: рендерится поверх, перекрывает первую
        // видимую строку (нормально для липкой шапки); клики ловит её
        // собственный on_mouse_down + stop_propagation.
        if let Some((content, colors)) = header {
            editor = editor.child(self.render_csv_sticky_header(pi, cx, &content, &colors));
        }
        // Панель поиска (6.1, D13): абсолютный оверлей top-right,
        // постоянная пока открыта; только в АКТИВНОЙ панели (File-scope
        // поиск привязан к её вкладке); фокус панели — клавиши
        // редактора не получают (ловушка D9.2), клики стопаются на
        // корне панели.
        if search_here && self.search.is_some() {
            editor = editor.child(self.render_search_panel(cx));
        }
        editor.into_any_element()
    }

    /// Липкая шапка CSV (D12.5): ABSOLUTE-копия строки-заголовка над
    /// списком (top 0, вся ширина контейнера, высота LINE_HEIGHT):
    /// гуттер с номером 1 + текстовая ячейка с ЦВЕТАМИ КОЛОНОК
    /// (split_line_styled по спанам строки 0; кареток/выделений в
    /// копии нет), сплошной фон (текст под ним не просвечивает),
    /// нижняя РАМКА border_b_1. Клик — прокрутка к началу
    /// (`scroll_to_item(0, Top)` + notify; предикат скрывает шапку).
    fn render_csv_sticky_header(
        &self,
        pi: usize,
        cx: &Context<Self>,
        content: &str,
        colors: &[Span],
    ) -> gpui::Stateful<gpui::Div> {
        // Текстовая ячейка — структура обычной строки (ловушка
        // gui_tests: пустых сегментов не плодим — caret-разрезов тут
        // нет, пустые тексты пропускаем). Горизонтальный скролл:
        // текст шапки ЕДЕТ вместе со строками (гуттер стоит) —
        // отрицательный margin на величину прокрутки (render читает
        // offset прошлого prepaint — ловушка D12.5, шапка догоняет).
        let scroll_x = -f32::from(self.panes[pi].h_scroll.offset().x).max(0.0);
        let mut text_cell = div()
            .flex()
            .items_center()
            .ml(px(-scroll_x))
            .text_size(px(self.font.size))
            .font_family(self.font.family.clone())
            .whitespace_nowrap();
        for (si, seg) in split_line_styled(content, &[], &[], colors).into_iter().enumerate() {
            if seg.text.is_empty() {
                continue;
            }
            let mut seg_div = div().child(seg.text);
            if let Some(class) = seg.class {
                seg_div = seg_div.text_color(rgb(crate::theme::current().syntax_color(class)));
            }
            let key = format!("csv-sticky-seg:{si}");
            seg_div = seg_div.debug_selector(move || key.clone());
            text_cell = text_cell.child(seg_div);
        }
        div()
            .id(("csv-sticky-header", pi))
            .absolute()
            .top_0()
            .left_0()
            // Ширина — до вертикальной дорожки (left+right, без
            // w_full): шапка не наезжает на полосу прокрутки.
            .right(px(crate::scrollbar::TRACK_W))
            .h(px(self.font.line_height()))
            .flex()
            .items_center()
            .bg(rgb(crate::theme::current().csv_header_bg))
            .border_b_1()
            .border_color(rgb(crate::theme::current().csv_header_border))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _e, _w, cx| {
                cx.stop_propagation();
                // Клик по шапке — к началу файла: строка 0 возвращается
                // на своё место, оверлей исчезает по предикату.
                this.panes[pi]
                    .scroll
                    .scroll_to_item(0, gpui::ScrollStrategy::Top);
                // Ловушка отложенного скролла (спайк D12.5):
                // scroll_to_item применяется в prepaint ПОСЛЕ render —
                // оверлей на этом кадре строится по СТАРОМУ offset и
                // висел бы до следующего события мыши. Прокрутка к
                // началу == offset (0, 0) — ставим немедленно (тот же
                // общий Rc<RefCell>, что читает render; deferred-скролл
                // подтвердит значение в prepaint).
                this.panes[pi]
                    .scroll
                    .0
                    .borrow()
                    .base_handle
                    .set_offset(point(px(0.0), px(0.0)));
                cx.notify();
            }))
            .debug_selector(|| "csv-sticky-header".to_string())
            // Гуттер как у обычной строки: [зона | номер 1 | зона]
            // (маркеры закладки/фолда в копии неактивны).
            .child(
                div()
                    .w(px(GUTTER_W))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .text_size(px(11.0))
                    .text_color(rgb(crate::theme::current().gutter))
                    .child(div().w(px(GUTTER_ZONE_W)).flex_shrink_0())
                    .child(div().flex_1().text_right().child("1"))
                    .child(div().w(px(GUTTER_ZONE_W)).flex_shrink_0()),
            )
            .child(text_cell)
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
