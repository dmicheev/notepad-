//! Палитра (VS Code-подобная тёмная; темы — задача 8.2). Все цвета UI
//! — в одном месте; литералы, встречавшиеся раньше прямо в рендерах,
//! получили имена.

// Основная палитра (была в workspace.rs).
pub(crate) const BG: u32 = 0x1e1e1e; // редактор
pub(crate) const PANEL: u32 = 0x252526; // боковая панель
pub(crate) const TABBAR: u32 = 0x2d2d2d; // фон таб-бара / неактивные вкладки
pub(crate) const ACCENT: u32 = 0x007acc; // активная вкладка (полоса), статус-бар
pub(crate) const CARET_LINE: u32 = 0x2a2d2e; // подсветка строки с кареткой
pub(crate) const CARET: u32 = 0xaeafad; // каретка
pub(crate) const GUTTER: u32 = 0x858585; // номера строк
pub(crate) const TEXT: u32 = 0xd4d4d4; // основной текст
pub(crate) const DIM: u32 = 0x6b6b6b; // плейсхолдеры
pub(crate) const CLOSE_HOVER: u32 = 0x3e3e42;
pub(crate) const SELECTION: u32 = 0x264f78; // фон выделенного текста

// Ранее безымянные литералы из рендеров (значения не менялись).
pub(crate) const TAB_INACTIVE: u32 = 0x9d9d9d; // текст неактивных вкладок, «×», кнопки
pub(crate) const DIRTY_DOT: u32 = 0xffcc66; // индикатор несохранённой вкладки
pub(crate) const STATUS_FG: u32 = 0xffffff; // текст статус-бара
pub(crate) const STATUS_DIM: u32 = 0xcfd8e6; // «notepad+» справа в статус-баре
pub(crate) const PANEL_HEADER: u32 = 0xbbbbbb; // заголовки секций левой панели
pub(crate) const PANEL_HINT: u32 = 0x4f4f52; // «coming in group 7»

// Палитра подсветки синтаксиса (5.2): классы editor-core::highlight →
// цвета Dark+ (как VS Code). Класс 0 (обычный текст) цвет не задаёт —
// сегмент наследует TEXT (round-trip: строка без подсветки рендерится
// бит-в-бит как раньше).
pub(crate) const SYN_KEYWORD: u32 = 0x569cd6; // fn, let, if (синий)
pub(crate) const SYN_TYPE: u32 = 0x4ec9b0; // типы, таблицы/ключи TOML (бирюзовый)
pub(crate) const SYN_STRING: u32 = 0xce9178; // строки (оранжевый)
pub(crate) const SYN_COMMENT: u32 = 0x6a9955; // комментарии (зелёный)
pub(crate) const SYN_NUMBER: u32 = 0xb5cea8; // числа (светло-зелёный)
pub(crate) const SYN_FUNCTION: u32 = 0xdcdcaa; // имена функций (жёлтый)
pub(crate) const SYN_CONSTANT: u32 = 0x4fc1ff; // константы/self/булевы (голубой)

// CSV/TSV (D12.4): колонки чередуются 4 цветами (Dark+ семейст­во,
// соседние колонки всегда различимы).
pub(crate) const SYN_COL_A: u32 = 0x9cdcfe; // колонки 0, 4, … (голубой)
pub(crate) const SYN_COL_B: u32 = 0xce9178; // колонки 1, 5, … (оранжевый)
pub(crate) const SYN_COL_C: u32 = 0xb5cea8; // колонки 2, 6, … (зелёный)
pub(crate) const SYN_COL_D: u32 = 0xd7ba7d; // колонки 3, 7, … (золотой)

// Липкая шапка CSV/TSV (D12.5): фон строки-заголовка (сплошной —
// текст под оверлеем не просвечивает) и рамка/разделитель снизу.
pub(crate) const CSV_HEADER_BG: u32 = 0x252526; // фон заголовка (как PANEL)
pub(crate) const CSV_HEADER_BORDER: u32 = 0x007acc; // линия-рамка (акцент)

/// Цвет класса подсветки (editor_core::highlight::StyleClass).
pub(crate) fn syntax_color(class: u8) -> u32 {
    match class {
        editor_core::highlight::CLASS_COMMENT => SYN_COMMENT,
        editor_core::highlight::CLASS_STRING => SYN_STRING,
        editor_core::highlight::CLASS_KEYWORD => SYN_KEYWORD,
        editor_core::highlight::CLASS_NUMBER => SYN_NUMBER,
        editor_core::highlight::CLASS_FUNCTION => SYN_FUNCTION,
        editor_core::highlight::CLASS_TYPE => SYN_TYPE,
        editor_core::highlight::CLASS_CONSTANT => SYN_CONSTANT,
        editor_core::highlight::CLASS_COLUMN_A => SYN_COL_A,
        editor_core::highlight::CLASS_COLUMN_B => SYN_COL_B,
        editor_core::highlight::CLASS_COLUMN_C => SYN_COL_C,
        editor_core::highlight::CLASS_COLUMN_D => SYN_COL_D,
        _ => TEXT,
    }
}
