//! Предпросмотр Markdown (change markdown-preview): панель справа от
//! области редактора, рендерит активную Markdown-вкладку и обновляется
//! при правках без сохранения (design D3–D6).
//!
//! Модель: `markdown::parse` (pulldown-cmark) → плоские `RenderItem`
//! (вложенность списков/цитат — отступом/полосой) — рендер через
//! виртуализованный `gpui::list` (переменные высоты блоков).
//!
//! Живое обновление (D4): memo по `(панель, вкладка, ревизия, длина)`;
//! первая сборка модели вкладки ≤ 2 МБ — синхронно (мгновенное
//! переключение), правки и большие файлы — debounce 200 мс +
//! парсинг в фоне (спека: видимость ≤ 300 мс после правки).
//!
//! Код-блоки: подсветка существующим `HighlightEngine` (syntect) по
//! токену языка fenced-строки; бюджеты: ≤ 400 строк на блок, ≤ 2000
//! строк подсветки на сборку модели (остальное — монохром).

use std::sync::Arc;
use std::time::Duration;

use gpui::{AnyElement, App, Context, HighlightStyle, InteractiveText, ListState, StyledText, div, prelude::*, px, rgb};
use markdown::{Align, Block, Inline};

use crate::actions::ToggleMarkdownPreview;
use crate::buffer_tab::BufferTab;
use crate::theme;
use crate::workspace::Workspace;

/// Debounce перед фоновым парсингом (D4; спека ≤ 300 мс).
const DEBOUNCE: Duration = Duration::from_millis(200);
/// Синхронная первая сборка модели — до этого размера (мгновенный
/// отклик при переключении вкладок).
const SYNC_PARSE_MAX: usize = 2 * 1024 * 1024;
/// Потолок строк кода для подсветки одного блока.
const CODE_BLOCK_LINES: usize = 400;
/// Потолок ПОДСВЕЧЕННЫХ строк кода на одну сборку модели.
const CODE_HIGHLIGHT_LINES: usize = 2000;
/// Моноширинный код в предпросмотре чуть мельче прозы.
const CODE_FONT_SIZE: f32 = 12.5;

/// Ключ актуальности модели: слот вкладки + ревизия + длина.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PreviewKey {
    pane: usize,
    tab: usize,
    revision: u64,
    len: usize,
}

/// Готовая к рендеру модель (плоские ряды).
struct PreviewModel {
    key: PreviewKey,
    items: Arc<Vec<RenderItem>>,
}

/// Состояние предпросмотра в Workspace.
pub(crate) struct MarkdownPreview {
    /// Панель открыта (тумблер Cmd/Ctrl+Shift+V, меню View, тулбар).
    pub open: bool,
    /// Ширина панели (px), выставленная драгом разделителя;
    /// `None` — пропорция по умолчанию (flex_basis 40%).
    width: Option<f32>,
    /// Идёт драг разделителя (мышь захвачена полноэкранным оверлеем).
    pub(crate) drag: bool,
    /// Скролл блочного списка (живёт между кадрами).
    scroll: ListState,
    /// Актуальная модель.
    model: Option<PreviewModel>,
    /// Запланирован debounce-пересчёт (ключ на момент планирования).
    scheduled: Option<PreviewKey>,
}

impl MarkdownPreview {
    pub(crate) fn new() -> Self {
        MarkdownPreview {
            open: false,
            width: None,
            drag: false,
            scroll: ListState::new(0, gpui::ListAlignment::Top, px(120.0)),
            model: None,
            scheduled: None,
        }
    }
}

/// Плоский ряд предпросмотра: один блок виртуализованного списка.
struct RenderItem {
    /// Уровень вложенности списков (отступ слева).
    indent: u16,
    /// Глубина цитаты (левая полоса).
    quote: u8,
    kind: ItemKind,
}

enum ItemKind {
    Heading { level: u8, inlines: Arc<Vec<Inline>> },
    Paragraph(Arc<Vec<Inline>>),
    Code { code: String, spans: Vec<Arc<Vec<editor_core::highlight::Span>>> },
    ListItem { marker: String, inlines: Arc<Vec<Inline>>, checked: Option<bool> },
    TableRow { cells: Arc<Vec<Arc<Vec<Inline>>>>, aligns: Arc<Vec<Align>>, header: bool },
    Rule,
    Html(String),
}

impl Workspace {
    /// Тумблер предпросмотра (меню View / Cmd+Shift+V / кнопка тулбара).
    pub(crate) fn on_toggle_markdown_preview(
        &mut self,
        _: &ToggleMarkdownPreview,
        _window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        self.markdown_preview.open = !self.markdown_preview.open;
        self.set_status_message(crate::i18n::t(
            cx,
            if self.markdown_preview.open { "status.preview_on" } else { "status.preview_off" },
        ), cx);
        cx.notify();
    }

    /// Активная вкладка — источник предпросмотра Markdown?
    fn markdown_source(&self) -> Option<(PreviewKey, &BufferTab)> {
        let tab = self.model().active()?;
        let key = PreviewKey {
            pane: self.active_pane,
            tab: self.model().active_index()?,
            revision: tab.doc.revision(),
            len: tab.doc.len_chars(),
        };
        if !is_markdown_tab(tab) {
            return None;
        }
        Some((key, tab))
    }

    /// Панель предпросмотра: placeholder для не-Markdown вкладок,
    /// иначе — виртуализованный список блоков (обновление модели —
    /// `ensure_preview_model` на каждом кадре).
    pub(crate) fn render_markdown_preview(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_preview_model(cx);
        let open_markdown = self.markdown_source().map(|(key, _)| key);
        let panel_bg = theme::current().panel;
        // Ширина: пропорция 40% по умолчанию ИЛИ явные px от драга
        // разделителя (min_w — страховка клампов preview_width_at).
        let mut base = div()
            .flex()
            .flex_col()
            .min_w(px(crate::geometry::PREVIEW_MIN_W))
            .bg(rgb(panel_bg))
            .debug_selector(|| "preview-panel".to_string());
        base = match self.markdown_preview.width {
            Some(w) => base.w(px(w)),
            None => base.flex_basis(gpui::relative(0.4)),
        };
        match (open_markdown, self.markdown_preview.model.as_ref()) {
            (None, _) => base
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .px_6()
                        .text_size(px(13.0))
                        .text_color(rgb(theme::current().dim))
                        .child(crate::i18n::t(cx, "preview.placeholder")),
                )
                .into_any_element(),
            (Some(_), None) => base.into_any_element(),
            (Some(_), Some(model)) => {
                let items = model.items.clone();
                let scroll = self.markdown_preview.scroll.clone();
                let mono = self.font.clone();
                base.child(
                    gpui::list(scroll, move |ix, _window, cx| {
                        render_row(ix, &items[ix], &mono, cx)
                    })
                    .flex_1()
                    .min_h_0()
                    .px_6()
                    .py_4(),
                )
                .into_any_element()
            }
        }
    }

    /// Разделитель [редактор | предпросмотр] (драг ширины): 1px линия
    /// в 5px хит-зоне (визуальная граница панелей та же — tabbar),
    /// ew-resize курсор; mouse down — захват + ширина = vw − мышь,
    /// двойной клик — сброс к пропорции 40%.
    pub(crate) fn render_preview_divider(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let dragging = self.markdown_preview.drag;
        let line = rgb(crate::theme::current().tabbar);
        let hover = rgb(crate::theme::current().caret_line);
        let accent = rgb(crate::theme::current().accent);
        div()
            .id("preview-divider")
            .debug_selector(|| "preview-divider".to_string())
            .w(px(crate::geometry::PREVIEW_DIVIDER_HIT_W))
            .flex_shrink_0()
            .relative()
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .when(dragging, |d| d.bg(accent))
            .hover(|d| d.bg(hover))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(
                    |this, e: &gpui::MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        // Двойной клик — сброс к пропорции 40% (на down:
                        // одиночный down уже захватывает мышь оверлеем,
                        // ClickEvent до разделителя не дойдёт).
                        if e.click_count >= 2 {
                            this.markdown_preview.width = None;
                            this.markdown_preview.drag = false;
                            cx.notify();
                            return;
                        }
                        let vw = f32::from(window.viewport_size().width);
                        this.markdown_preview.drag = true;
                        this.markdown_preview.width = Some(crate::geometry::preview_width_at(
                            f32::from(e.position.x),
                            vw,
                        ));
                        cx.notify();
                    },
                ),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px((crate::geometry::PREVIEW_DIVIDER_HIT_W - 1.0) / 2.0))
                    .w(px(1.0))
                    .bg(line),
            )
            .into_any_element()
    }

    /// Полноэкранный захват мыши на время драга разделителя (мышь
    /// может уйти за пределы центральной зоны — на дерево/статус);
    /// move/up потребляются здесь. Паттерн драга — scrollbar
    /// (thumb_drag): move без зажатой кнопки — пропущенный up,
    /// завершаем сами (self-healing).
    pub(crate) fn render_preview_drag_overlay(&mut self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("preview-drag-overlay")
            .absolute()
            .size_full()
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .on_mouse_move(
                cx.listener(|this, e: &gpui::MouseMoveEvent, window, cx| {
                    if !this.markdown_preview.drag {
                        return;
                    }
                    cx.stop_propagation();
                    if e.pressed_button != Some(gpui::MouseButton::Left) {
                        this.markdown_preview.drag = false;
                        cx.notify();
                        return;
                    }
                    let vw = f32::from(window.viewport_size().width);
                    this.markdown_preview.width = Some(crate::geometry::preview_width_at(
                        f32::from(e.position.x),
                        vw,
                    ));
                    cx.notify();
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _e: &gpui::MouseUpEvent, _window, cx| {
                    cx.stop_propagation();
                    if this.markdown_preview.drag {
                        this.markdown_preview.drag = false;
                        cx.notify();
                    }
                }),
            )
            .into_any_element()
    }

    /// Живое обновление (D4): модель актуальна — ничего; первая сборка
    /// вкладки ≤ 2 МБ — синхронно; иначе debounce + фон.
    fn ensure_preview_model(&mut self, cx: &mut Context<Self>) {
        let Some((key, tab)) = self.markdown_source() else {
            return;
        };
        if self.markdown_preview.model.as_ref().is_some_and(|m| m.key == key) {
            return;
        }
        // Первая модель этой вкладки (переключение/открытие) — сразу.
        let same_tab = self
            .markdown_preview
            .model
            .as_ref()
            .is_some_and(|m| m.key.pane == key.pane && m.key.tab == key.tab);
        if !same_tab && key.len <= SYNC_PARSE_MAX {
            let blocks = markdown::parse(&tab.doc.text());
            self.set_preview_model(key, blocks);
            cx.notify();
            return;
        }
        // Правки (и большие первые сборки): debounce, один таймер.
        if self.markdown_preview.scheduled.is_none() {
            self.schedule_preview_parse(cx);
        }
    }

    /// Назначить отложенный парсинг (200 мс, сброс правками — сверкой
    /// ключа на срабатывании).
    fn schedule_preview_parse(&mut self, cx: &mut Context<Self>) {
        let Some((key, _)) = self.markdown_source() else { return };
        self.markdown_preview.scheduled = Some(key);
        cx.spawn(async move |this, cx| {
            enum Step {
                Parse(PreviewKey, String),
                Wait,
                Stop,
            }
            loop {
                cx.background_executor().timer(DEBOUNCE).await;
                let step = this
                    .update(cx, |ws, _cx| {
                        // Снимок без удержания займа: ключ + текст.
                        let snap = ws.markdown_source().map(|(k, t)| (k, t.doc.text()));
                        match (snap, ws.markdown_preview.scheduled) {
                            (Some((live, text)), Some(s)) if s == live => {
                                ws.markdown_preview.scheduled = None;
                                Step::Parse(live, text)
                            }
                            // Правки продолжились — тишина 200 мс заново.
                            (Some((live, _)), Some(_)) => {
                                ws.markdown_preview.scheduled = Some(live);
                                Step::Wait
                            }
                            _ => {
                                ws.markdown_preview.scheduled = None;
                                Step::Stop
                            }
                        }
                    })
                    .unwrap_or(Step::Stop);
                match step {
                    Step::Stop => return,
                    Step::Wait => continue,
                    Step::Parse(key, text) => {
                        let blocks = cx
                            .background_executor()
                            .spawn(async move { markdown::parse(&text) })
                            .await;
                        let alive = this
                            .update(cx, |ws, cx| {
                                // Слот вкладки не изменился — принимаем;
                                // ревизия могла уйти вперёд: следующий
                                // кадр перепланирует (дебаунс продолжится).
                                let same_slot = ws
                                    .markdown_source()
                                    .is_some_and(|(k, _)| k.pane == key.pane && k.tab == key.tab);
                                if same_slot {
                                    ws.set_preview_model(key, blocks);
                                    cx.notify();
                                }
                            })
                            .is_ok();
                        if !alive {
                            return;
                        }
                        // Пока парсили, могли запланировать новый
                        // пересчёт — продолжаем цикл ожидания.
                        let again = this
                            .update(cx, |ws, _cx| ws.markdown_preview.scheduled.is_some())
                            .unwrap_or(false);
                        if !again {
                            return;
                        }
                    }
                }
            }
        })
        .detach();
    }

    /// Собрать модель (flatten + подсветка код-блоков) и обновить
    /// счётчик списка; смена вкладки — скролл наверх.
    fn set_preview_model(&mut self, key: PreviewKey, blocks: Vec<Block>) {
        let mut highlighter = CodeHighlighter::default();
        let mut items = Vec::new();
        flatten(&blocks, 0, 0, &mut items, &mut highlighter);
        let new_len = items.len();
        let old_len = self.markdown_preview.model.as_ref().map(|m| m.items.len()).unwrap_or(0);
        let tab_switched = !self
            .markdown_preview
            .model
            .as_ref()
            .is_some_and(|m| m.key.pane == key.pane && m.key.tab == key.tab);
        self.markdown_preview.scroll.splice(0..old_len, new_len);
        if tab_switched {
            self.markdown_preview.scroll.scroll_to_reveal_item(0);
        }
        self.markdown_preview.model = Some(PreviewModel { key, items: Arc::new(items) });
    }
}

/// Критерий «вкладка — Markdown» (D3): расширение или выбранный язык
/// (тот же источник, что у подсветки).
fn is_markdown_tab(tab: &BufferTab) -> bool {
    if let Some(path) = &tab.path
        && let Some(ext) = path.extension().and_then(|e| e.to_str())
    {
        let ext = ext.to_ascii_lowercase();
        if ext == "md" || ext == "markdown" {
            return true;
        }
    }
    tab.language.label() == "Markdown"
}

/// Рекурсивное выравнивание блоков в плоские ряды.
fn flatten(
    blocks: &[Block],
    indent: u16,
    quote: u8,
    out: &mut Vec<RenderItem>,
    hl: &mut CodeHighlighter,
) {
    for block in blocks {
        match block {
            Block::Heading { level, inlines } => out.push(RenderItem {
                indent,
                quote,
                kind: ItemKind::Heading { level: *level, inlines: Arc::new(inlines.clone()) },
            }),
            Block::Paragraph(inlines) => out.push(RenderItem {
                indent,
                quote,
                kind: ItemKind::Paragraph(Arc::new(inlines.clone())),
            }),
            Block::CodeBlock { lang, code } => out.push(RenderItem {
                indent,
                quote,
                kind: ItemKind::Code {
                    code: code.clone(),
                    spans: hl.highlight(lang.as_deref(), code),
                },
            }),
            Block::List { ordered, items } => {
                for (i, item) in items.iter().enumerate() {
                    let marker = match item.checked {
                        Some(true) => "☑".to_string(),
                        Some(false) => "☐".to_string(),
                        None if *ordered => format!("{}.", i + 1),
                        None => "•".to_string(),
                    };
                    if !item.inlines.is_empty() {
                        out.push(RenderItem {
                            indent,
                            quote,
                            kind: ItemKind::ListItem {
                                marker,
                                inlines: Arc::new(item.inlines.clone()),
                                checked: item.checked,
                            },
                        });
                    }
                    flatten(&item.children, indent + 1, quote, out, hl);
                }
            }
            Block::Quote(inner) => flatten(inner, indent, quote + 1, out, hl),
            Block::Table { aligns, rows } => {
                let aligns = Arc::new(aligns.clone());
                for (ri, row) in rows.iter().enumerate() {
                    out.push(RenderItem {
                        indent,
                        quote,
                        kind: ItemKind::TableRow {
                            cells: Arc::new(row.iter().map(|c| Arc::new(c.clone())).collect()),
                            aligns: aligns.clone(),
                            header: ri == 0,
                        },
                    });
                }
            }
            Block::Rule => out.push(RenderItem { indent, quote, kind: ItemKind::Rule }),
            Block::Html(html) => {
                out.push(RenderItem { indent, quote, kind: ItemKind::Html(html.clone()) })
            }
        }
    }
}

/// Подсветка код-блоков с бюджетом строк (D5).
#[derive(Default)]
struct CodeHighlighter {
    spent: usize,
}

impl CodeHighlighter {
    fn highlight(&mut self, lang: Option<&str>, code: &str) -> Vec<Arc<Vec<editor_core::highlight::Span>>> {
        let lines = code.lines().count();
        if lines == 0
            || lines > CODE_BLOCK_LINES
            || self.spent + lines > CODE_HIGHLIGHT_LINES
            || lang.is_none_or(str::is_empty)
        {
            return Vec::new();
        }
        let Some(syntax) = editor_core::syntax::syntax_set()
            .find_syntax_by_token(lang.unwrap_or_default())
        else {
            return Vec::new();
        };
        let doc = editor_core::Document::from_text(code);
        let mut engine = editor_core::HighlightEngine::new();
        engine.set_syntax(Some(syntax));
        engine.set_budget(Duration::from_millis(20));
        let total = doc.len_lines();
        engine.ensure_window(&doc, 0, total);
        self.spent += lines;
        (0..total)
            .map(|l| engine.spans(l).cloned().unwrap_or_default())
            .collect()
    }
}

/// Собранный для `StyledText` строчный контент.
struct ShapedInline {
    text: String,
    /// Диапазоны (в char-байтах строки) со стилями.
    highlights: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    /// Кликабельные диапазоны (индекс → URL).
    links: Vec<(std::ops::Range<usize>, String)>,
}

/// Инлайны → плоская строка + стили + ссылки (D5: StyledText с
/// `with_highlights`; картинки — чип-плейсхолдер с alt).
fn shape_inlines(inlines: &[Inline]) -> ShapedInline {
    let theme = theme::current();
    let mut out = ShapedInline { text: String::new(), highlights: Vec::new(), links: Vec::new() };
    let push = |out: &mut ShapedInline, chunk: &str, style: Option<HighlightStyle>| {
        let start = out.text.len();
        out.text.push_str(chunk);
        if let Some(style) = style {
            out.highlights.push((start..out.text.len(), style));
        }
    };
    for inline in inlines {
        match inline {
            Inline::Text(t, marks) => {
                let mut style = HighlightStyle::default();
                if marks.bold {
                    style.font_weight = Some(gpui::FontWeight::BOLD);
                }
                if marks.italic {
                    style.font_style = Some(gpui::FontStyle::Italic);
                }
                if marks.strike {
                    style.strikethrough = Some(gpui::StrikethroughStyle { thickness: px(1.0), color: None });
                }
                if marks.code {
                    style.background_color =
                        Some(gpui::hsla(0.0, 0.0, 0.25, 0.35));
                    style.color = Some(gpui::rgb(theme.syn_constant).into());
                }
                push(&mut out, t, Some(style));
            }
            Inline::Link { text, url } => {
                let start = out.text.len();
                out.text.push_str(text);
                let end = out.text.len();
                out.highlights.push((
                    start..end,
                    HighlightStyle {
                        color: Some(gpui::rgb(theme.preview_link).into()),
                        underline: Some(gpui::UnderlineStyle {
                            thickness: px(1.0),
                            color: Some(gpui::rgb(theme.preview_link).into()),
                            wavy: false,
                        }),
                        ..Default::default()
                    },
                ));
                out.links.push((start..end, url.clone()));
            }
            Inline::Image { alt, url } => {
                // Плейсхолдер (спека): загрузка изображений — Non-Goal.
                let chip = format!("🖼 {alt} ");
                let style = HighlightStyle {
                    font_style: Some(gpui::FontStyle::Italic),
                    color: Some(gpui::rgb(theme.dim).into()),
                    ..Default::default()
                };
                let _ = url;
                push(&mut out, &chip, Some(style));
            }
            Inline::SoftBreak => push(&mut out, " ", None),
            Inline::HardBreak => push(&mut out, "\n", None),
        }
    }
    out
}

/// Рендер одного ряда списка.
fn render_row(ix: usize, item: &RenderItem, mono: &crate::geometry::FontConfig, cx: &App) -> AnyElement {
    let theme = theme::current();
    let pad_left = px(item.indent as f32 * 18.0 + if item.quote > 0 { 10.0 } else { 0.0 });
    let mut row = div()
        .id(("md-row", ix))
        .debug_selector(move || format!("md-row-{ix}"))
        .flex()
        .flex_col()
        .pl(pad_left)
        .min_w_0()
        .py_1();
    if item.quote > 0 {
        row = row
            .border_l_2()
            .border_color(rgb(theme.preview_quote_bar))
            .pl(pad_left + px(8.0));
    }
    match &item.kind {
        ItemKind::Heading { level, inlines } => {
            let size = match level {
                1 => 24.0,
                2 => 20.0,
                3 => 17.0,
                4 => 15.0,
                5 => 14.0,
                _ => 13.0,
            };
            let shaped = shape_inlines(inlines);
            row = row
                .text_size(px(size))
                .text_color(rgb(theme.text))
                .mt_3()
                .mb_1()
                .child(styled_text(ix, shaped, *level, true));
        }
        ItemKind::Paragraph(inlines) => {
            let shaped = shape_inlines(inlines);
            row = row
                .text_size(px(14.0))
                .text_color(rgb(theme.text))
                                .child(styled_text(ix, shaped, 0, false));
        }
        ItemKind::ListItem { marker, inlines, checked } => {
            let shaped = shape_inlines(inlines);
            let marker_color = if checked.is_some() { theme.accent } else { theme.gutter };
            row = row
                .flex_row()
                .text_size(px(14.0))
                .text_color(rgb(theme.text))
                                .child(
                    div()
                        .w(px(22.0))
                        .flex_shrink_0()
                        .text_color(rgb(marker_color))
                        .child(marker.clone()),
                )
                .child(styled_text(ix, shaped, 0, false));
        }
        ItemKind::Code { code, spans } => {
            let lines: Vec<&str> = code.lines().collect();
            let mut rows = div().flex().flex_col().my_1().p_3().rounded_md().bg(rgb(theme.preview_code_bg));
            for (li, line) in lines.iter().enumerate() {
                let line = (*line).to_string() + "\n";
                let mut text = StyledText::new(line.clone());
                if let Some(spans) = spans.get(li)
                    && !spans.is_empty()
                {
                    text = text.with_highlights(code_span_highlights(&line, spans, theme));
                }
                rows = rows.child(
                    div()
                        .font_family(mono.family.clone())
                        .text_size(px(CODE_FONT_SIZE))
                        .text_color(rgb(theme.text))
                        .child(text),
                );
            }
            row = row.child(rows);
        }
        ItemKind::TableRow { cells, aligns, header } => {
            let mut cols = div().flex().w_full();
            if *header {
                cols = cols
                    .border_t_1()
                    .border_b_1()
                    .border_color(rgb(theme.gutter))
                    .text_color(rgb(theme.text));
            } else {
                cols = cols.border_b_1().border_color(rgb(theme.caret_line));
            }
            for (ci, cell) in cells.iter().enumerate() {
                let shaped = shape_inlines(cell);
                let mut cell_div = div().flex_1().min_w_0().px_2().py_1();
                cell_div = match aligns.get(ci).copied().unwrap_or(Align::None) {
                    Align::Left => cell_div.justify_start(),
                    Align::Center => cell_div.justify_center(),
                    Align::Right => cell_div.justify_end(),
                    Align::None => cell_div,
                };
                let text = styled_text(ix, shaped, 0, *header);
                cols = cols.child(cell_div.child(
                    div().text_size(px(13.0)).child(text),
                ));
            }
            row = row.child(cols);
        }
        ItemKind::Rule => {
            row = row.child(div().my_3().h(px(1.0)).bg(rgb(theme.gutter)));
        }
        ItemKind::Html(html) => {
            row = row.child(
                div()
                    .font_family(mono.family.clone())
                    .text_size(px(12.0))
                    .text_color(rgb(theme.dim))
                    .child(html.clone()),
            );
        }
    }
    let _ = cx;
    row.into_any_element()
}

/// Подсветка код-строки для StyledText: спаны движка — CHAR-координаты
/// от начала строки (highlight.rs), а `with_highlights` ждёт БАЙТОВЫЕ
/// границы — прямой прокинутый char-диапазон в multibyte-строке (//
/// комментарий) ронял gpui-панику `is_char_boundary(run.end)`.
/// Кламп в конец строки — спаны и линии синхронны, но кламп дешевле
/// паники. Редактор делает тот же перевод (split_line_full/byte_at).
fn code_span_highlights(
    text: &str,
    spans: &[editor_core::highlight::Span],
    theme: &crate::theme::Theme,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    let byte_of: Vec<usize> = text
        .char_indices()
        .map(|(b, _)| b)
        .chain(std::iter::once(text.len()))
        .collect();
    let to_byte = |ix: u32| byte_of.get(ix as usize).copied().unwrap_or(text.len());
    spans
        .iter()
        .filter(|s| s.class != 0)
        .filter(|s| s.end > s.start)
        .map(|s| {
            (
                to_byte(s.start)..to_byte(s.end),
                HighlightStyle {
                    color: Some(gpui::rgb(theme.syntax_color(s.class)).into()),
                    ..Default::default()
                },
            )
        })
        .collect()
}

/// `StyledText` (кликабельные ссылки — `InteractiveText`, D5/D7:
/// открываем только http(s), прочие схемы — стилизованы без действия).
fn styled_text(ix: usize, shaped: ShapedInline, heading: u8, force_bold: bool) -> AnyElement {
    let text = StyledText::new(shaped.text.clone()).with_highlights(
        shaped
            .highlights
            .iter()
            .map(|(range, style)| {
                let mut style = *style;
                if force_bold || heading > 0 {
                    style.font_weight = Some(gpui::FontWeight::BOLD);
                }
                (range.clone(), style)
            })
            .collect::<Vec<_>>(),
    );
    if shaped.links.is_empty() {
        return text.into_any_element();
    }
    let urls: Vec<String> = shaped.links.iter().map(|(_, url)| url.clone()).collect();
    let ranges: Vec<std::ops::Range<usize>> =
        shaped.links.iter().map(|(range, _)| range.clone()).collect();
    InteractiveText::new(("md-inline", ix), text)
        .on_click(ranges, move |ix, _window, cx| {
            if let Some(url) = urls.get(ix)
                && (url.starts_with("http://") || url.starts_with("https://"))
            {
                cx.open_url(url);
            }
        })
        .into_any_element()
}

#[cfg(test)]
impl Workspace {
    /// Число рядов модели предпросмотра (GUI-тесты 3.3/3.4): None —
    /// модели нет (вкладка не Markdown / ещё не построена).
    pub(crate) fn preview_item_count(&self) -> Option<usize> {
        self.markdown_preview.model.as_ref().map(|m| m.items.len())
    }

    /// Ревизия, на которой построена модель (GUI-тест живого
    /// обновления: рост = debounce-пересборка состоялась).
    pub(crate) fn markdown_preview_revision(&self) -> u64 {
        self.markdown_preview.model.as_ref().map(|m| m.key.revision).unwrap_or(0)
    }

    /// Ширина панели предпросмотра, выставленная драгом разделителя
    /// (GUI-тест драга/сброса; None — пропорция по умолчанию).
    pub(crate) fn markdown_preview_width_for_test(&self) -> Option<f32> {
        self.markdown_preview.width
    }
}

/// Критерий Markdown-вкладки для GUI-теста следования за вкладкой.
#[cfg(test)]
pub(crate) fn is_markdown_tab_for_test(tab: &BufferTab) -> bool {
    is_markdown_tab(tab)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D3: критерий Markdown — по расширению (регистронезависимо) и
    /// по метке языка вкладки.
    #[test]
    fn markdown_criterion_by_extension() {
        let mut tab = BufferTab::untitled();
        tab.path = Some("/tmp/README.md".into());
        assert!(is_markdown_tab(&tab));
        tab.path = Some("/tmp/notes.MARKDOWN".into());
        assert!(is_markdown_tab(&tab));
        tab.path = Some("/tmp/main.rs".into());
        assert!(!is_markdown_tab(&tab));
        tab.path = None;
        assert!(!is_markdown_tab(&tab)); // untitled Plain Text
    }

    /// D5: плоское выравнивание — вложенный список в цитате даёт ряды с
    /// отступом и полосой; подсветка кода не выходит за бюджеты.
    #[test]
    fn flatten_nested_and_budgets() {
        let blocks = markdown::parse("> цитата\n>\n> - пункт\n>   - глубже\n\n```rust\nfn a() {}\n```\n");
        let mut hl = CodeHighlighter::default();
        let mut items = Vec::new();
        flatten(&blocks, 0, 0, &mut items, &mut hl);
        // quote-параграф (quote=1), list-ряд (quote=1), вложенный
        // (quote=1, indent=1), код-блок (spans пустые — syntect-набор
        // не инициализирован в юнит-контексте, бюджеты соблюдает).
        assert!(items.iter().any(|i| i.quote == 1 && i.indent == 1));
        assert!(hl.spent <= CODE_HIGHLIGHT_LINES);
    }

    /// D5: шейпер собирает стили/ссылки с корректными диапазонами.
    #[test]
    fn shape_builds_ranges() {
        let inlines = vec![
            Inline::Text("см. ".into(), markdown::Marks::default()),
            Inline::Link { text: "док".into(), url: "https://x.dev".into() },
        ];
        let shaped = shape_inlines(&inlines);
        assert_eq!(shaped.text, "см. док");
        assert_eq!(shaped.links.len(), 1);
        // Диапазоны — байтовые ("см. " = 6 байт UTF-8), как ждёт
        // StyledText::with_highlights.
        assert_eq!(shaped.links[0].0, 6..12);
        assert!(!shaped.highlights.is_empty());
    }

    /// Спаны кода — char-координаты, with_highlights ждёт байтовые
    /// границы: multibyte-комментарий в блоке раньше ронял панику
    /// `is_char_boundary(run.end)` в gpui (баг ручной проверки).
    #[test]
    fn code_span_highlights_multibyte_byte_ranges() {
        use editor_core::highlight::{CLASS_COMMENT, Span};
        let line = "// комментарий\n"; // 3 байта "// " + 11×2 байт + \n
        assert_eq!(line.chars().count(), 15); // 14 символов + терминатор
        let spans = vec![
            Span { start: 0, end: 14, class: CLASS_COMMENT },
            Span { start: 0, end: 0, class: CLASS_COMMENT }, // пустой — мимо
        ];
        let ranges: Vec<_> = code_span_highlights(line, &spans, theme::current())
            .into_iter()
            .map(|(r, _)| r)
            .collect();
        assert_eq!(ranges, vec![0..line.len() - 1], "вся строка без \\n");
        for r in &ranges {
            assert!(line.is_char_boundary(r.start) && line.is_char_boundary(r.end));
        }
        // Спан за пределами строки — кламп в конец, не паника.
        let over = vec![Span { start: 0, end: 999, class: CLASS_COMMENT }];
        let ranges: Vec<_> = code_span_highlights(line, &over, theme::current())
            .into_iter()
            .map(|(r, _)| r)
            .collect();
        assert_eq!(ranges, vec![0..line.len()]);
    }
}
