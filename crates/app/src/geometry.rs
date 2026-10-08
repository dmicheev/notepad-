//! Геометрия редактора (единый источник, направление B рефакторинга):
//! константы размеров + пересчёт координат мыши/страниц. Раньше высоты
//! таб-бара/статус-бара были захардкожены в `page_lines` и рендерах
//! панелей — теперь каждый размер живёт здесь.

use gpui::{Window, px};

use crate::workspace::Workspace;

/// Высота строки редактора при шрифте ПО УМОЛЧАНИЮ (px; gui-тесты).
pub(crate) const LINE_HEIGHT: f32 = 20.0;
/// Минимальный/максимальный размер шрифта (8.3, зум Cmd+=/Cmd+-).
pub(crate) const FONT_MIN: f32 = 9.0;
pub(crate) const FONT_MAX: f32 = 28.0;
/// Ширина гуттера: зоны закладки и фолда + номер строки (px).
pub(crate) const GUTTER_W: f32 = 56.0;
/// Ширина одной зоны гуттера — закладка или фолд (px).
pub(crate) const GUTTER_ZONE_W: f32 = 14.0;
/// Ширина левой панели «Project» (px).
pub(crate) const PANEL_W: f32 = 220.0;
/// Минимальная ширина панели предпросмотра Markdown (px).
pub(crate) const PREVIEW_MIN_W: f32 = 320.0;
/// Минимальная ширина области редактора при драге разделителя
/// предпросмотра (px).
pub(crate) const EDITOR_MIN_W: f32 = 400.0;
/// Ширина хит-зоны разделителя предпросмотра (px); визуальная
/// линия внутри — 1px.
pub(crate) const PREVIEW_DIVIDER_HIT_W: f32 = 5.0;
/// Высота таб-бара (px).
pub(crate) const TABBAR_H: f32 = 32.0;
/// Высота статус-бара (px).
pub(crate) const STATUS_H: f32 = 24.0;
/// Настройка шрифта редактора (8.3): семейство + размер, применяется
/// немедленно (char_advance сбрасывается), сохраняется в сессии.
#[derive(Clone, Debug, PartialEq)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig {
            family: DEFAULT_FONT.to_string(),
            size: DEFAULT_FONT_SIZE,
        }
    }
}

impl FontConfig {
    /// Высота строки (px): пропорция дефолта 13px→20px (~1.54em);
    /// 13 → 20, 14 → 22, 16 → 25.
    pub fn line_height(&self) -> f32 {
        (self.size * 20.0 / 13.0).round().max(8.0)
    }
}

/// Шрифт редактора ПО УМОЛЧАНИЮ (моноширинный macOS); тесты
/// используют дефолтные величины (LINE_HEIGHT ниже).
pub(crate) const DEFAULT_FONT: &str = "Menlo";
/// Размер шрифта по умолчанию (px).
pub(crate) const DEFAULT_FONT_SIZE: f32 = 13.0;

impl Workspace {
    // ---- Шрифт редактора (8.3): семейство/размер/высота строки ----

    /// Высота строки при текущем шрифте (8.3; константа LINE_HEIGHT —
    /// только дефолт для тестов).
    pub(crate) fn line_height(&self) -> f32 {
        self.font.line_height()
    }

    /// Применить шрифт немедленно: сброс измеренного advance
    /// (следующий кадр перемеряет), перерисовка.
    pub(crate) fn apply_font(&mut self, font: FontConfig, cx: &mut gpui::Context<Self>) {
        if font.family.trim().is_empty() {
            return;
        }
        self.font = font;
        self.char_advance = None;
        cx.notify();
    }

    /// Зум (8.3/8.5): Cmd+= / Cmd+- / Cmd+0 — размер ±1 (клампы),
    /// сброс к дефолту; применяется немедленно.
    pub(crate) fn zoom_font(&mut self, delta: isize, cx: &mut gpui::Context<Self>) {
        let mut font = self.font.clone();
        if delta == 0 {
            font.size = DEFAULT_FONT_SIZE;
        } else if delta > 0 {
            font.size = (font.size + 1.0).min(FONT_MAX);
        } else {
            font.size = (font.size - 1.0).max(FONT_MIN);
        }
        self.apply_font(font, cx);
    }

    // ---- Координаты мыши → позиция в документе (4.2, D9.2) ----

    /// Bounds uniform_list АКТИВНОЙ панели в координатах окна (после
    /// первой отрисовки).
    pub(crate) fn editor_bounds(&self) -> Option<gpui::Bounds<gpui::Pixels>> {
        let b = self.pane().scroll.0.borrow().base_handle.bounds();
        (b.size.width > gpui::Pixels::ZERO && b.size.height > gpui::Pixels::ZERO).then_some(b)
    }

    /// Ширина char: измеряется один раз (до смены шрифта — 8.3) —
    /// advance «0» выбранного моноширинного шрифта через
    /// `text_system().ch_advance()`; иначе fallback.
    pub(crate) fn char_advance(&mut self, window: &Window) -> f32 {
        if let Some(a) = self.char_advance {
            return a;
        }
        let advance = measure_char_advance(window, &self.font);
        self.char_advance = Some(advance);
        advance
    }

    /// Видимый индекс строки по y окна (clamp в список); real-строка
    /// через мапу (в 4.2 — тождественная, folding — 4.5). Считается по
    /// АКТИВНОЙ панели (обработчики мыши активируют панель до вызова).
    pub(crate) fn visible_line_at_y(&self, y: f32) -> Option<usize> {
        let b = self.editor_bounds()?;
        let offset = f32::from(self.pane().scroll.0.borrow().base_handle.offset().y);
        let vis =
            (((y - f32::from(b.origin.y) - offset) / self.line_height()).floor() as i64).max(0)
                as usize;
        Some(vis.min(self.visible_len().saturating_sub(1)))
    }

    /// Колонка по x окна: (x − гуттер) / char_advance, округление к
    /// ближайшей границе символа, clamp по длине строки делает модель.
    /// Горизонтальный скролл учтён АВТОМАТИЧЕСКИ: bounds uniform_list
    /// (ребёнок зоны скролла) уже сдвинуты на offset.x (gpui
    /// `with_element_offset` в prepaint детей) — явно вычитать нельзя
    /// (двойной счёт).
    pub(crate) fn col_at_x(&mut self, window: &Window, x: f32) -> usize {
        let b = self.editor_bounds();
        let left = b.map(|b| f32::from(b.origin.x)).unwrap_or(0.0);
        let advance = self.char_advance(window);
        let local = (x - left - GUTTER_W).max(0.0);
        (local / advance).round() as usize
    }

    /// Высота страницы прокрутки в строках: фактическая высота
    /// uniform_list (если отрисован) или окно минус таб-бар и статус-бар.
    /// Страница PgUp/PgDn = всё видимое пространство строк (D9.2).
    pub(crate) fn page_lines(&self, window: &Window) -> usize {
        let h = self
            .editor_bounds()
            .map(|b| f32::from(b.size.height))
            .unwrap_or_else(|| {
                f32::from(window.viewport_size().height) - TABBAR_H - STATUS_H
            });
        ((h / self.line_height()).floor() as usize).max(1)
    }
}

/// Видимость липкой шапки CSV (D12.5): строка 0 полностью ушла из
/// вида — `|offset.y| ≥ LINE_HEIGHT` (offset.y ОТРИЦАТЕЛЕН при
/// прокрутке вниз — см. `visible_line_at_y`; положительный —
/// овэрскролл-отскок вверх, строка 0 видна). Частичный показ
/// (0 < |offset| < LINE_HEIGHT) шапку НЕ дублирует — дубль поверх
/// наполовину видной строки 0 мерцает.
pub(crate) fn sticky_header_visible(offset_y: f32) -> bool {
    offset_y.abs() >= LINE_HEIGHT
}

/// Ширина предпросмотра из позиции мыши на разделителе: правый край
/// окна − x (панель упирается в правый край окна), клампы
/// [PREVIEW_MIN_W, vw − EDITOR_MIN_W]. Двойной кламп max — окно
/// уже суммы минимумов: отдаём приоритет предпросмотру (редактор
/// остаётся с гибким min_w_0 строки).
pub(crate) fn preview_width_at(mouse_x: f32, viewport_w: f32) -> f32 {
    (viewport_w - mouse_x).clamp(PREVIEW_MIN_W, (viewport_w - EDITOR_MIN_W).max(PREVIEW_MIN_W))
}

/// Измерение advance «0» выбранного шрифта; fallback 0.6em, если
/// семейство не нашлось/текстовая система недоступна (D9.2).
fn measure_char_advance(window: &Window, font: &FontConfig) -> f32 {
    let ts = window.text_system();
    ts.ch_advance(ts.resolve_font(&gpui::font(font.family.clone())), px(font.size))
        .map(f32::from)
        .ok()
        .filter(|a| *a > 0.0)
        .unwrap_or(font.size * 0.6)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D12.5: предикат липкой шапки — ровно граница «строка 0 ушла
    /// целиком» (замер --> GUI-тест csv_sticky_header_pins_on_scroll).
    #[test]
    fn sticky_header_visibility_predicate() {
        assert!(!sticky_header_visible(0.0), "начало файла — шапки нет");
        assert!(
            !sticky_header_visible(-LINE_HEIGHT + 0.01),
            "частичный показ строки 0 — шапку НЕ дублируем (мерцание)"
        );
        assert!(sticky_header_visible(-LINE_HEIGHT), "полный уход — шапка");
        assert!(sticky_header_visible(-400.0), "глубокая прокрутка — шапка");
        assert!(
            !sticky_header_visible(12.0),
            "овэрскролл-отскок вверх — строка 0 видна, шапки нет"
        );
    }

    /// Клампы ширины предпросмотра при драге разделителя: мышь →
    /// ширина (правый край − x), минимумы панели/редактора, узкое
    /// окно — приоритет минимуму предпросмотра.
    #[test]
    fn preview_width_clamps() {
        // Свободная зона: ширина = vw − x.
        assert_eq!(preview_width_at(600.0, 1200.0), 600.0);
        // Мышь у правого края — минимум панели.
        assert_eq!(preview_width_at(1100.0, 1200.0), PREVIEW_MIN_W);
        // Мышь далеко влево — редактору остаётся EDITOR_MIN_W.
        assert_eq!(preview_width_at(100.0, 1200.0), 1200.0 - EDITOR_MIN_W);
        // Окно уже суммы минимумов — клампы не переворачиваются.
        let w = preview_width_at(0.0, 500.0);
        assert_eq!(w, PREVIEW_MIN_W);
        assert!(w <= 500.0);
    }
}
