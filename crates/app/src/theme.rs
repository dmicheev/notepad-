//! Палитры тем (8.2/D15.2): `Theme` — все цвета UI в одном месте;
//! `DARK` (VS Code Dark+) и `LIGHT` (Light+). Выбор — System
//! (по системной настройке через `Window::appearance`) / Dark / Light;
//! хранится в Workspace (`theme_kind`), применяется мгновенной
//! перерисовкой, сохраняется в сессии. Подсветка синтаксиса — те же
//! семейства Dark+/Light+.

/// Тема: все цвета UI (значения Dark+/Light+). Поля — бывшие константы
/// palette-модуля; литералы, встречавшиеся в рендерах, получили имена.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Theme {
    /// Фон редактора.
    pub bg: u32,
    /// Боковая панель.
    pub panel: u32,
    /// Фон таб-бара / неактивные вкладки / разделители панелей.
    pub tabbar: u32,
    /// Акцент: активная вкладка (полоса), статус-бар.
    pub accent: u32,
    /// Подсветка строки с кареткой.
    pub caret_line: u32,
    /// Каретка.
    pub caret: u32,
    /// Номера строк.
    pub gutter: u32,
    /// Основной текст.
    pub text: u32,
    /// Плейсхолдеры.
    pub dim: u32,
    /// Ховер кнопок закрытия/иконок.
    pub close_hover: u32,
    /// Фон выделенного текста.
    pub selection: u32,
    /// Текст неактивных вкладок, «×», кнопки.
    pub tab_inactive: u32,
    /// Индикатор несохранённой вкладки.
    pub dirty_dot: u32,
    /// Текст статус-бара.
    pub status_fg: u32,
    /// «notepad+» справа в статус-баре.
    pub status_dim: u32,
    /// Заголовки секций левой панели.
    pub panel_header: u32,
    /// Подписи-заглушки секций.
    pub panel_hint: u32,
    // Подсветка синтаксиса (классы editor-core::highlight → цвета
    // Dark+/Light+; класс 0 — обычный текст, наследует `text`).
    pub syn_keyword: u32,
    pub syn_type: u32,
    pub syn_string: u32,
    pub syn_comment: u32,
    pub syn_number: u32,
    pub syn_function: u32,
    pub syn_constant: u32,
    // CSV/TSV: колонки чередуются 4 цветами (соседние всегда различимы).
    pub syn_col_a: u32,
    pub syn_col_b: u32,
    pub syn_col_c: u32,
    pub syn_col_d: u32,
    // Липкая шапка CSV/TSV (D12.5): сплошной фон + рамка-акцент.
    pub csv_header_bg: u32,
    pub csv_header_border: u32,
    // Поиск и замена: совпадения — оранжевое семейство, текущее ярче;
    // ошибка regex — красный.
    pub search_match: u32,
    pub search_current: u32,
    pub search_error: u32,
    /// Ховер кликабельных зон (язык в статус-бара, чипы поиска).
    pub status_lang_hover: u32,
    // Предпросмотр Markdown (markdown-preview D5): ссылка, фон
    // код-чипа/блока, полоса цитаты.
    pub preview_link: u32,
    pub preview_code_bg: u32,
    pub preview_quote_bar: u32,
    // Полосы прокрутки: ползунок (ховер — светлее/темнее), дорожки
    // редактора — своя зона у текста.
    pub scrollbar_thumb: u32,
    pub scrollbar_thumb_hover: u32,
    pub scrollbar_track: u32,
    pub scrollbar_track_border: u32,
}

/// Тёмная тема (VS Code Dark+) — бывшие константы модуля, значения
/// не менялись.
pub(crate) static DARK: Theme = Theme {
    bg: 0x1e1e1e,
    panel: 0x252526,
    tabbar: 0x2d2d2d,
    accent: 0x007acc,
    caret_line: 0x2a2d2e,
    caret: 0xaeafad,
    gutter: 0x858585,
    text: 0xd4d4d4,
    dim: 0x6b6b6b,
    close_hover: 0x3e3e42,
    selection: 0x264f78,
    tab_inactive: 0x9d9d9d,
    dirty_dot: 0xffcc66,
    status_fg: 0xffffff,
    status_dim: 0xcfd8e6,
    panel_header: 0xbbbbbb,
    panel_hint: 0x4f4f52,
    syn_keyword: 0x569cd6,
    syn_type: 0x4ec9b0,
    syn_string: 0xce9178,
    syn_comment: 0x6a9955,
    syn_number: 0xb5cea8,
    syn_function: 0xdcdcaa,
    syn_constant: 0x4fc1ff,
    syn_col_a: 0x9cdcfe,
    syn_col_b: 0xce9178,
    syn_col_c: 0xb5cea8,
    syn_col_d: 0xd7ba7d,
    csv_header_bg: 0x252526,
    csv_header_border: 0x007acc,
    search_match: 0x9e6a03,
    search_current: 0xd78a12,
    search_error: 0xf14c4c,
    status_lang_hover: 0x3d9be8,
    preview_link: 0x4db2ff,
    preview_code_bg: 0x2a2d2e,
    preview_quote_bar: 0x6a737d,
    scrollbar_thumb: 0x4a4a4c,
    scrollbar_thumb_hover: 0x636366,
    scrollbar_track: 0x1a1a1b,
    scrollbar_track_border: 0x2d2d2d,
};

/// Светлая тема (VS Code Light+).
pub(crate) static LIGHT: Theme = Theme {
    bg: 0xffffff,
    panel: 0xf3f3f3,
    tabbar: 0xe8e8e8,
    accent: 0x007acc,
    caret_line: 0xf0f0f0,
    caret: 0x000000,
    gutter: 0x6e7681,
    text: 0x24292e,
    dim: 0x8a9199,
    close_hover: 0xdcdcdc,
    selection: 0xadd6ff,
    tab_inactive: 0x6f6f6f,
    dirty_dot: 0xb26500,
    status_fg: 0xffffff,
    status_dim: 0xd6e4f5,
    panel_header: 0x555555,
    panel_hint: 0xb0b0b0,
    syn_keyword: 0x0000ff,
    syn_type: 0x267f99,
    syn_string: 0xa31515,
    syn_comment: 0x008000,
    syn_number: 0x098658,
    syn_function: 0x795e26,
    syn_constant: 0x001080,
    syn_col_a: 0x001080,
    syn_col_b: 0xa31515,
    syn_col_c: 0x098658,
    syn_col_d: 0x8f5900,
    csv_header_bg: 0xe8e8e8,
    csv_header_border: 0x007acc,
    search_match: 0xffe792,
    search_current: 0xffd24a,
    search_error: 0xcd3131,
    status_lang_hover: 0x1f7fd6,
    preview_link: 0x0563c1,
    preview_code_bg: 0xf0f0f0,
    preview_quote_bar: 0x9aa0a6,
    scrollbar_thumb: 0xc1c1c1,
    scrollbar_thumb_hover: 0xa8a8a8,
    scrollbar_track: 0xf5f5f5,
    scrollbar_track_border: 0xe0e0e0,
};

impl Theme {
    /// Цвет класса подсветки (editor_core::highlight::StyleClass);
    /// класс 0 (обычный текст) цвет не задаёт — сегмент наследует
    /// `text` (round-trip: строка без подсветки рендерится как раньше).
    pub(crate) fn syntax_color(&self, class: u8) -> u32 {
        match class {
            editor_core::highlight::CLASS_COMMENT => self.syn_comment,
            editor_core::highlight::CLASS_STRING => self.syn_string,
            editor_core::highlight::CLASS_KEYWORD => self.syn_keyword,
            editor_core::highlight::CLASS_NUMBER => self.syn_number,
            editor_core::highlight::CLASS_FUNCTION => self.syn_function,
            editor_core::highlight::CLASS_TYPE => self.syn_type,
            editor_core::highlight::CLASS_CONSTANT => self.syn_constant,
            editor_core::highlight::CLASS_COLUMN_A => self.syn_col_a,
            editor_core::highlight::CLASS_COLUMN_B => self.syn_col_b,
            editor_core::highlight::CLASS_COLUMN_C => self.syn_col_c,
            editor_core::highlight::CLASS_COLUMN_D => self.syn_col_d,
            _ => self.text,
        }
    }
}

/// Выбор темы (8.2): System — следовать системной настройке.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeKind {
    #[default]
    System,
    Dark,
    Light,
}

impl ThemeKind {
    /// Строка сессии ("system"/"dark"/"light").
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeKind::System => "system",
            ThemeKind::Dark => "dark",
            ThemeKind::Light => "light",
        }
    }

    /// Из строки сессии (неизвестное — System).
    pub fn parse(s: &str) -> Self {
        match s {
            "dark" => ThemeKind::Dark,
            "light" => ThemeKind::Light,
            _ => ThemeKind::System,
        }
    }

    /// Разрешить в палитру по внешнему виду окна (System → системная).
    pub fn resolve(self, appearance: gpui::WindowAppearance) -> &'static Theme {
        let dark = match self {
            ThemeKind::Dark => true,
            ThemeKind::Light => false,
            ThemeKind::System => matches!(appearance, gpui::WindowAppearance::Dark),
        };
        if dark { &DARK } else { &LIGHT }
    }
}

/// Текущая (разрешённая) тема: глобальный выбор приложения
/// (одиночное окно MVP; выставляется на каждом кадре из
/// `Workspace::theme_kind` × системная настройка).
static CURRENT: std::sync::RwLock<&'static Theme> = std::sync::RwLock::new(&DARK);

/// Текущая палитра (чтение дёшево — RwLock).
pub(crate) fn current() -> &'static Theme {
    *CURRENT.read().unwrap()
}

/// Установить текущую палитру (переключение темы, 8.2).
pub(crate) fn set_current(theme: &'static Theme) {
    *CURRENT.write().unwrap() = theme;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 8.2: parse/as_str — round-trip; неизвестное — System.
    #[test]
    fn theme_kind_roundtrip() {
        for k in [ThemeKind::System, ThemeKind::Dark, ThemeKind::Light] {
            assert_eq!(ThemeKind::parse(k.as_str()), k);
        }
        assert_eq!(ThemeKind::parse("nonsense"), ThemeKind::System);
    }

    /// 8.2: System следует appearance; ручные режимы фиксированы.
    #[test]
    fn theme_kind_resolve_follows_system() {
        assert!(std::ptr::eq(
            ThemeKind::System.resolve(gpui::WindowAppearance::Dark),
            &DARK
        ));
        assert!(std::ptr::eq(
            ThemeKind::System.resolve(gpui::WindowAppearance::Light),
            &LIGHT
        ));
        assert!(std::ptr::eq(
            ThemeKind::Dark.resolve(gpui::WindowAppearance::Light),
            &DARK
        ));
        assert!(std::ptr::eq(
            ThemeKind::Light.resolve(gpui::WindowAppearance::Dark),
            &LIGHT
        ));
    }
}
