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
