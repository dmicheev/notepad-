//! Модель отрендеренного Markdown-документа (change markdown-preview,
//! design D2): чистые типы без GUI-зависимостей — парсер (`parse.rs`)
//! превращает события pulldown-cmark в эту модель, рендер живёт в app.

mod parse;

pub use parse::parse;

/// Блок документа.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// Заголовок уровня 1–6.
    Heading { level: u8, inlines: Vec<Inline> },
    /// Абзац.
    Paragraph(Vec<Inline>),
    /// Блок кода: ``` ```lang ``` ``` или с отступом. `lang` — первый
    /// токен info-строки (может быть неизвестным рендеру).
    CodeBlock { lang: Option<String>, code: String },
    /// Список: `ordered` — нумерованный. Пункты несут контент и
    /// вложенные блоки (подсписки — детьми, не в общем потоке).
    List { ordered: bool, items: Vec<ListItem> },
    /// Цитата: содержимое — вложенные блоки.
    Quote(Vec<Block>),
    /// Таблица GFM: `rows[0]` — шапка; выравнивания по колонкам.
    Table { aligns: Vec<Align>, rows: Vec<Vec<Vec<Inline>>> },
    /// Горизонтальная линия (`---`, `***`, `___`).
    Rule,
    /// Блочный HTML: рендерится как текст (исполнение — Non-Goal).
    Html(String),
}

/// Пункт списка.
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    /// Строчное содержимое пункта.
    pub inlines: Vec<Inline>,
    /// Вложенные блоки (подсписки, цитаты, код).
    pub children: Vec<Block>,
    /// Чек-бокс (GFM task list): `None` — обычный пункт.
    pub checked: Option<bool>,
}

/// Выравнивание колонки таблицы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

/// Строчный элемент.
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    /// Текст с метками стиля (стек emph/strong/strike свёрнут в Marks
    /// при парсинге — рендер получает готовые флаги).
    Text(String, Marks),
    /// Ссылка: показываемый текст + URL.
    Link { text: String, url: String },
    /// Изображение: alt-текст + URL (MVP-рендер — плейсхолдер).
    Image { alt: String, url: String },
    /// Мягкий перенос (внутри абзаца).
    SoftBreak,
    /// Жёсткий перенос (конец строки с двумя пробелами / `<br>`).
    HardBreak,
}

/// Метки стиля текста (битовые — могут сочетаться).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    /// Моноширинный инлайн-код.
    pub code: bool,
}
