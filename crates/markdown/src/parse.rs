//! Парсер Markdown → модель (change markdown-preview, design D1/D2):
//! поток событий pulldown-cmark (CommonMark 0.31 + таблицы/
//! зачёркивание/чек-листы GFM) сворачивается в [`Vec<Block>`] за один
//! проход, без построения AST. Вложенные стили инлайнов схлопываются
//! в [`Marks`] (счётчики strong/emphasis/strike), ссылки/изображения
//! собирают внутренний текст в плоскую строку. Предохранитель глубины
//! [`MAX_DEPTH`] защищает от патологических вложенностей.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::{Align, Block, Inline, ListItem, Marks};

/// Потолок вложенности блоков (список в цитате в списке…): глубже —
/// содержимое пропускается (потребляется без построения модели).
pub const MAX_DEPTH: usize = 64;

/// Расширения парсера: GFM-таблицы, зачёркивание, чек-листы.
fn options() -> Options {
    Options::ENABLE_TABLES.union(Options::ENABLE_STRIKETHROUGH).union(Options::ENABLE_TASKLISTS)
}

/// Разобрать Markdown-текст в модель документа.
pub fn parse(text: &str) -> Vec<Block> {
    let events = Parser::new_ext(text, options()).peekable();
    let mut builder = Builder { events, depth: 0, strong: 0, emph: 0, strike: 0 };
    builder.blocks()
}

/// Построитель модели поверх потока событий. Соглашение о границах:
/// [`Builder::blocks`] останавливается на ближайшем `End`, НЕ потребляя
/// его; владелец контейнера потребляет свой `End` сам.
struct Builder<'a> {
    events: std::iter::Peekable<Parser<'a>>,
    depth: usize,
    /// Счётчики открытых стилей (вложенные `***` = strong+emph).
    strong: u32,
    emph: u32,
    strike: u32,
}

impl<'a> Builder<'a> {
    /// Блоки до конца контейнера (или входа).
    fn blocks(&mut self) -> Vec<Block> {
        let mut out = Vec::new();
        while let Some(event) = self.events.peek() {
            if matches!(event, Event::End(_)) {
                break;
            }
            if let Some(block) = self.block() {
                out.push(block);
            }
        }
        out
    }

    /// Один блок; `None` — событие не дало блока (пропущено).
    fn block(&mut self) -> Option<Block> {
        let start = match self.events.next()? {
            Event::Start(tag) => tag,
            Event::Rule => return Some(Block::Rule),
            // Текст/инлайны вне блоков (нет CommonMark-пути, но поток
            // не должен ломаться) — собрать в абзац.
            other => return self.stray_inline(other),
        };
        if self.depth >= MAX_DEPTH {
            if let Some(end) = tag_end(&start) {
                self.skip_to_end(end);
            }
            return None;
        }
        self.depth += 1;
        let block = self.block_body(start);
        self.depth -= 1;
        block
    }

    fn block_body(&mut self, tag: Tag<'_>) -> Option<Block> {
        match tag {
            Tag::Paragraph => {
                let inlines = self.inlines();
                self.expect_end(TagEnd::Paragraph);
                Some(Block::Paragraph(inlines))
            }
            Tag::Heading { level, .. } => {
                let inlines = self.inlines();
                self.expect_end(TagEnd::Heading(level));
                Some(Block::Heading { level: heading_level(level), inlines })
            }
            Tag::BlockQuote(_) => {
                let inner = self.blocks();
                self.expect_end(TagEnd::BlockQuote(None));
                Some(Block::Quote(inner))
            }
            Tag::CodeBlock(kind) => {
                let mut code = String::new();
                for event in self.events.by_ref() {
                    match event {
                        Event::Text(t) | Event::Html(t) => code.push_str(&t),
                        Event::End(TagEnd::CodeBlock) => break,
                        _ => {}
                    }
                }
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().map(str::to_string)
                    }
                    CodeBlockKind::Indented => None,
                };
                Some(Block::CodeBlock { lang, code })
            }
            Tag::List(start) => {
                let ordered = start.is_some();
                let items = self.list_items();
                self.expect_end(TagEnd::List(start.is_some()));
                Some(Block::List { ordered, items })
            }
            Tag::Table(aligns) => {
                let aligns = aligns.iter().map(|a| align(*a)).collect();
                let mut rows = Vec::new();
                while let Some(event) = self.events.peek() {
                    match event {
                        Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => {
                            let is_head = matches!(event, Event::Start(Tag::TableHead));
                            self.events.next();
                            let mut cells = Vec::new();
                            for _ in 0.. {
                                if !matches!(self.events.peek(), Some(Event::Start(Tag::TableCell))) {
                                    break;
                                }
                                self.events.next();
                                cells.push(self.inlines());
                                self.expect_end(TagEnd::TableCell);
                            }
                            self.expect_end(if is_head {
                                TagEnd::TableHead
                            } else {
                                TagEnd::TableRow
                            });
                            rows.push(cells);
                        }
                        Event::End(TagEnd::Table) => {
                            self.events.next();
                            break;
                        }
                        _ => break, // защита от битого потока
                    }
                }
                Some(Block::Table { aligns, rows })
            }
            Tag::HtmlBlock => {
                let mut html = String::new();
                for event in self.events.by_ref() {
                    match event {
                        Event::Html(t) | Event::Text(t) => html.push_str(&t),
                        Event::End(TagEnd::HtmlBlock) => break,
                        _ => {}
                    }
                }
                Some(Block::Html(html))
            }
            // Прочие контейнеры (footnote-определения, …): содержимое
            // парсим как блоки — модель их не различает.
            other => {
                if let Some(end) = tag_end(&other) {
                    let inner = self.blocks();
                    self.expect_end(end);
                    return inner.into_iter().next();
                }
                None
            }
        }
    }

    /// Пункты списка: строчный контент первого абзаца — в `inlines`
    /// пункта, подсписки/цитаты/код — в `children`.
    fn list_items(&mut self) -> Vec<ListItem> {
        let mut items = Vec::new();
        while matches!(self.events.peek(), Some(Event::Start(Tag::Item))) {
            self.events.next(); // Start(Item)
            self.depth += 1;
            let mut item = ListItem { inlines: Vec::new(), children: Vec::new(), checked: None };
            if let Some(Event::TaskListMarker(checked)) = self.events.peek() {
                item.checked = Some(*checked);
                self.events.next();
            }
            // Строчный контент до первого блока (tight-список: Text без
            // Paragraph; loose: Start(Paragraph)).
            if let Some(Event::Start(Tag::Paragraph)) = self.events.peek() {
                self.events.next();
                if item.inlines.is_empty() {
                    item.inlines = self.inlines();
                }
                self.expect_end(TagEnd::Paragraph);
            } else if !matches!(self.events.peek(), Some(Event::End(TagEnd::Item))) {
                item.inlines = self.inlines();
            }
            // Остальное — вложенные блоки (подсписки приходят без
            // обёртки: List внутри Item).
            while !matches!(self.events.peek(), Some(Event::End(TagEnd::Item))) {
                match self.events.peek() {
                    Some(Event::Start(Tag::List(_))) => {
                        if let Some(Block::List { ordered, items: sub }) = self.block() {
                            item.children.push(Block::List { ordered, items: sub });
                        }
                    }
                    _ => {
                        if let Some(block) = self.block() {
                            item.children.push(block);
                        }
                    }
                }
            }
            self.events.next(); // End(Item)
            self.depth -= 1;
            items.push(item);
        }
        items
    }

    /// Инлайны до конца контейнера (его `End` остаётся во входе).
    /// Блочные контейнеры (подсписок/цитата/код внутри пункта) НЕ
    /// потребляются — их разбирает блочный цикл владельца.
    fn inlines(&mut self) -> Vec<Inline> {
        let mut out = Vec::new();
        while let Some(event) = self.events.peek() {
            if matches!(event, Event::End(_)) {
                break;
            }
            if let Event::Start(tag) = event
                && is_block_tag(tag)
            {
                break;
            }
            match self.events.next().expect("peeked") {
                Event::Text(t) => out.push(Inline::Text(t.into_string(), self.marks())),
                Event::Code(t) => {
                    let mut marks = self.marks();
                    marks.code = true;
                    out.push(Inline::Text(t.into_string(), marks));
                }
                Event::InlineHtml(t) => out.push(Inline::Text(t.into_string(), self.marks())),
                Event::SoftBreak => out.push(Inline::SoftBreak),
                Event::HardBreak => out.push(Inline::HardBreak),
                Event::Start(Tag::Strong) => {
                    self.strong += 1;
                    out.append(&mut self.inlines());
                    self.expect_end(TagEnd::Strong);
                    self.strong -= 1;
                }
                Event::Start(Tag::Emphasis) => {
                    self.emph += 1;
                    out.append(&mut self.inlines());
                    self.expect_end(TagEnd::Emphasis);
                    self.emph -= 1;
                }
                Event::Start(Tag::Strikethrough) => {
                    self.strike += 1;
                    out.append(&mut self.inlines());
                    self.expect_end(TagEnd::Strikethrough);
                    self.strike -= 1;
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    let inner = self.inlines();
                    self.expect_end(TagEnd::Link);
                    out.push(Inline::Link { text: plain_text(&inner), url: dest_url.into_string() });
                }
                Event::Start(Tag::Image { dest_url, .. }) => {
                    let inner = self.inlines();
                    self.expect_end(TagEnd::Image);
                    out.push(Inline::Image { alt: plain_text(&inner), url: dest_url.into_string() });
                }
                // Вложенные блоки внутри строчного контекста (нестандарт)
                // — пропускаем содержимое как текст.
                Event::Start(tag) => {
                    if let Some(end) = tag_end(&tag) {
                        let inner = self.inlines();
                        self.expect_end(end);
                        out.push(Inline::Text(plain_text(&inner), self.marks()));
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Событие вне блока: собрать как абзац (страховка потока).
    fn stray_inline(&mut self, event: Event<'_>) -> Option<Block> {
        let mut inlines = self.inlines();
        if let Some(last) = inlines.last_mut() {
            prepend_event_text(last, event);
        } else {
            inlines.push(Inline::Text(event_text(event)?, Marks::default()));
        }
        Some(Block::Paragraph(inlines))
    }

    fn marks(&self) -> Marks {
        Marks {
            bold: self.strong > 0,
            italic: self.emph > 0,
            strike: self.strike > 0,
            code: false,
        }
    }

    /// Потребить ожидаемый `End` (несовпадение — пропустить до него:
    /// поток pulldown-cmark корректен, защита только от наших ошибок).
    fn expect_end(&mut self, end: TagEnd) {
        if matches!(self.events.peek(), Some(Event::End(_))) {
            self.events.next();
            return;
        }
        self.skip_to_end(end);
    }

    /// Потребить события до `end` (включительно), считая вложенность.
    fn skip_to_end(&mut self, end: TagEnd) {
        let mut nesting = 0usize;
        for event in self.events.by_ref() {
            match event {
                Event::Start(_) => nesting += 1,
                Event::End(e) => {
                    if e == end && nesting == 0 {
                        return;
                    }
                    nesting = nesting.saturating_sub(1);
                }
                _ => {}
            }
        }
    }
}

/// `TagEnd` контейнера (для skip-режима предохранителя глубины).
fn tag_end(tag: &Tag<'_>) -> Option<TagEnd> {
    Some(match tag {
        Tag::Paragraph => TagEnd::Paragraph,
        Tag::Heading { level, .. } => TagEnd::Heading(*level),
        Tag::BlockQuote(kind) => TagEnd::BlockQuote(*kind),
        Tag::CodeBlock(_) => TagEnd::CodeBlock,
        Tag::List(start) => TagEnd::List(start.is_some()),
        Tag::Item => TagEnd::Item,
        Tag::Table(_) => TagEnd::Table,
        Tag::TableHead => TagEnd::TableHead,
        Tag::TableRow => TagEnd::TableRow,
        Tag::TableCell => TagEnd::TableCell,
        Tag::HtmlBlock => TagEnd::HtmlBlock,
        _ => return None,
    })
}

/// Тег открывает блочный контейнер (инлайны перед ним останавливаются).
fn is_block_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::CodeBlock(_)
            | Tag::List(_)
            | Tag::Item
            | Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::HtmlBlock
    )
}

fn heading_level(level: pulldown_cmark::HeadingLevel) -> u8 {
    use pulldown_cmark::HeadingLevel::*;
    match level {
        H1 => 1,
        H2 => 2,
        H3 => 3,
        H4 => 4,
        H5 => 5,
        H6 => 6,
    }
}

fn align(a: Alignment) -> Align {
    match a {
        Alignment::None => Align::None,
        Alignment::Left => Align::Left,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::Right,
    }
}

/// Плоский текст инлайнов (для текста ссылок и alt изображений).
fn plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(t, _) => out.push_str(t),
            Inline::Link { text, .. } => out.push_str(text),
            Inline::Image { alt, .. } => out.push_str(alt),
            Inline::SoftBreak | Inline::HardBreak => out.push(' '),
        }
    }
    out
}

/// Текст одиночного события (страховка `stray_inline`).
fn event_text(event: Event<'_>) -> Option<String> {
    match event {
        Event::Text(t) | Event::Code(t) | Event::InlineHtml(t) => Some(t.into_string()),
        Event::SoftBreak | Event::HardBreak => Some(" ".to_string()),
        _ => None,
    }
}

fn prepend_event_text(inline: &mut Inline, event: Event<'_>) {
    if let Some(text) = event_text(event)
        && let Inline::Text(existing, _) = inline
    {
        *existing = format!("{text}{existing}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Marks;

    fn text(s: &str) -> Inline {
        Inline::Text(s.to_string(), Marks::default())
    }
    fn marked(s: &str, marks: Marks) -> Inline {
        Inline::Text(s.to_string(), marks)
    }

    /// 1.2: заголовки 1–6 и абзацы.
    #[test]
    fn headings_and_paragraphs() {
        let doc = parse("# Один\n\n## Два\n\n### Три\n\nтекст абзаца");
        assert_eq!(
            doc,
            vec![
                Block::Heading { level: 1, inlines: vec![text("Один")] },
                Block::Heading { level: 2, inlines: vec![text("Два")] },
                Block::Heading { level: 3, inlines: vec![text("Три")] },
                Block::Paragraph(vec![text("текст абзаца")]),
            ]
        );
    }

    /// 1.2: инлайн-стили, включая вложенные (`***` = жирный+курсив).
    #[test]
    fn inline_marks() {
        let doc = parse("обычный **жирный** *курсив* ~~зачёркнутый~~ `код` ***обоих***");
        let Block::Paragraph(inlines) = &doc[0] else { panic!("paragraph") };
        let bold = Marks { bold: true, ..Default::default() };
        let italic = Marks { italic: true, ..Default::default() };
        let strike = Marks { strike: true, ..Default::default() };
        let code = Marks { code: true, ..Default::default() };
        let both = Marks { bold: true, italic: true, ..Default::default() };
        assert_eq!(inlines[0], text("обычный "));
        assert_eq!(inlines[1], marked("жирный", bold));
        assert_eq!(inlines[2], text(" "));
        assert_eq!(inlines[3], marked("курсив", italic));
        assert_eq!(inlines[5], marked("зачёркнутый", strike));
        assert_eq!(inlines[7], marked("код", code));
        assert_eq!(inlines[9], marked("обоих", both));
    }

    /// 1.2: soft/hard break.
    #[test]
    fn breaks() {
        let doc = parse("строка один\nстрока два  \nстрока три");
        let Block::Paragraph(inlines) = &doc[0] else { panic!("paragraph") };
        assert_eq!(inlines[1], Inline::SoftBreak);
        assert_eq!(inlines[3], Inline::HardBreak);
    }

    /// 1.3: код-блоки — fenced с языком, fenced без, с отступом.
    #[test]
    fn code_blocks() {
        let doc = parse("```rust\nfn main() {}\n```\n\n```\nplain\n```\n\n    indented\n");
        assert_eq!(
            doc[0],
            Block::CodeBlock { lang: Some("rust".into()), code: "fn main() {}\n".into() }
        );
        assert_eq!(doc[1], Block::CodeBlock { lang: None, code: "plain\n".into() });
        assert_eq!(doc[2], Block::CodeBlock { lang: None, code: "indented\n".into() });
    }

    /// 1.3: списки — маркированный, нумерованный, вложенный, чек-лист.
    #[test]
    fn lists() {
        let doc = parse("- a\n- b\n  - b1\n  - b2\n- c");
        assert_eq!(
            doc[0],
            Block::List {
                ordered: false,
                items: vec![
                    ListItem { inlines: vec![text("a")], children: vec![], checked: None },
                    ListItem {
                        inlines: vec![text("b")],
                        children: vec![Block::List {
                            ordered: false,
                            items: vec![
                                ListItem { inlines: vec![text("b1")], children: vec![], checked: None },
                                ListItem { inlines: vec![text("b2")], children: vec![], checked: None },
                            ],
                        }],
                        checked: None,
                    },
                    ListItem { inlines: vec![text("c")], children: vec![], checked: None },
                ],
            }
        );

        let doc = parse("1. первый\n2. второй");
        assert_eq!(doc[0], Block::List { ordered: true, items: vec![
            ListItem { inlines: vec![text("первый")], children: vec![], checked: None },
            ListItem { inlines: vec![text("второй")], children: vec![], checked: None },
        ]});

        let doc = parse("- [x] сделано\n- [ ] план");
        let Block::List { items, .. } = &doc[0] else { panic!("list") };
        assert_eq!(items[0].checked, Some(true));
        assert_eq!(items[1].checked, Some(false));
    }

    /// 1.3: цитата с вложенным списком, список с цитатой.
    #[test]
    fn quotes_nesting() {
        let doc = parse("> цитата\n>\n> - пункт\n> - ещё");
        assert_eq!(
            doc[0],
            Block::Quote(vec![
                Block::Paragraph(vec![text("цитата")]),
                Block::List {
                    ordered: false,
                    items: vec![
                        ListItem { inlines: vec![text("пункт")], children: vec![], checked: None },
                        ListItem { inlines: vec![text("ещё")], children: vec![], checked: None },
                    ],
                },
            ])
        );

        let doc = parse("- пункт\n  > вложенная цитата");
        let Block::List { items, .. } = &doc[0] else { panic!("list") };
        assert_eq!(
            items[0].children,
            vec![Block::Quote(vec![Block::Paragraph(vec![text("вложенная цитата")])])]
        );
    }

    /// 1.3: таблица с выравниваниями; шапка — rows[0].
    #[test]
    fn tables() {
        let doc = parse("| L | C | R | N |\n|:-:|-:|:-|-|\n| 1 | 2 | 3 | 4 |");
        assert_eq!(
            doc[0],
            Block::Table {
                aligns: vec![Align::Center, Align::Right, Align::Left, Align::None],
                rows: vec![
                    vec![vec![text("L")], vec![text("C")], vec![text("R")], vec![text("N")]],
                    vec![vec![text("1")], vec![text("2")], vec![text("3")], vec![text("4")]],
                ],
            }
        );
    }

    /// 1.3: `---`, сырой HTML-блок.
    #[test]
    fn rule_and_html() {
        let doc = parse("до\n\n---\n\n<div>html</div>\n");
        assert_eq!(doc[1], Block::Rule);
        assert_eq!(doc[2], Block::Html("<div>html</div>\n".into()));
    }

    /// 1.4: ссылки (текст — плоский), изображения (alt).
    #[test]
    fn links_and_images() {
        let doc = parse("[обычная](https://a.dev) [**жирная** ссылка](https://b.dev) ![Логотип](logo.png)");
        let Block::Paragraph(inlines) = &doc[0] else { panic!("paragraph") };
        assert_eq!(
            inlines[0],
            Inline::Link { text: "обычная".into(), url: "https://a.dev".into() }
        );
        assert_eq!(
            inlines[2],
            Inline::Link { text: "жирная ссылка".into(), url: "https://b.dev".into() }
        );
        assert_eq!(
            inlines[4],
            Inline::Image { alt: "Логотип".into(), url: "logo.png".into() }
        );
    }

    /// 1.4: предохранитель глубины — патологическая вложенность не
    /// роняет парсер и не переполняет стек.
    #[test]
    fn depth_guard() {
        let mut deep = String::new();
        for _ in 0..10_000 {
            deep.push_str("> ");
        }
        deep.push_str("дно");
        let doc = parse(&deep); // не завис и не упал
        assert!(!doc.is_empty());

        // Большой файл (2 МБ) разбирается за разумное время.
        let big = "абзац текста\n\n".repeat(120_000);
        let started = std::time::Instant::now();
        let doc = parse(&big);
        assert_eq!(doc.len(), 120_000);
        assert!(started.elapsed().as_secs() < 10, "2 МБ парсится < 10 с");
    }

    /// 1.5: эталонный документ со всеми конструкциями спеки.
    #[test]
    fn snapshot_full_document() {
        let src = "# Заголовок\n\nАбзац с **жирным** и [ссылкой](https://example.com).\n\n```rust\nfn main() {}\n```\n\n- пункт 1\n- пункт 2\n  1. вложенный\n\n> цитата\n\n| A | B |\n|---|:-:|\n| 1 | 2 |\n\n---\n\n![Картинка](x.png)\n\n<div>x</div>\n";
        let doc = parse(src);
        assert_eq!(doc.len(), 9, "{doc:#?}");
        assert_eq!(doc[0], Block::Heading { level: 1, inlines: vec![text("Заголовок")] });
        assert!(matches!(doc[1], Block::Paragraph(_)));
        assert!(matches!(&doc[2], Block::CodeBlock { lang: Some(l), .. } if l == "rust"));
        assert!(matches!(&doc[3], Block::List { ordered: false, items } if items.len() == 2
            && items[1].children.len() == 1));
        assert!(matches!(&doc[4], Block::Quote(_)));
        assert!(matches!(&doc[5], Block::Table { aligns, rows } if aligns[1] == Align::Center
            && rows.len() == 2));
        assert_eq!(doc[6], Block::Rule);
        assert!(matches!(&doc[7], Block::Paragraph(inl)
            if matches!(inl[0], Inline::Image { .. })));
        assert!(matches!(&doc[8], Block::Html(_)));
    }

    /// Пустой ввод → пустая модель.
    #[test]
    fn empty() {
        assert_eq!(parse(""), Vec::<Block>::new());
        assert_eq!(parse("   \n\n\n"), Vec::<Block>::new());
    }
}
