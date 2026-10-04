//! Геометрия редактора (единый источник, направление B рефакторинга):
//! константы размеров + пересчёт координат мыши/страниц. Раньше высоты
//! таб-бара/статус-бара были захардкожены в `page_lines` и рендерах
//! панелей — теперь каждый размер живёт здесь.

use gpui::{Window, px};

use crate::workspace::Workspace;

/// Высота строки редактора (px).
pub(crate) const LINE_HEIGHT: f32 = 20.0;
/// Ширина гуттера: зоны закладки и фолда + номер строки (px).
pub(crate) const GUTTER_W: f32 = 56.0;
/// Ширина одной зоны гуттера — закладка или фолд (px).
pub(crate) const GUTTER_ZONE_W: f32 = 14.0;
/// Ширина левой панели «Project» (px).
pub(crate) const PANEL_W: f32 = 220.0;
/// Высота таб-бара (px).
pub(crate) const TABBAR_H: f32 = 32.0;
/// Высота статус-бара (px).
pub(crate) const STATUS_H: f32 = 24.0;
/// Шрифт редактора (моноширинный).
pub(crate) const FONT: &str = "Menlo";
/// Размер шрифта редактора (px).
pub(crate) const FONT_SIZE: f32 = 13.0;
/// Заранее измерить не удалось (текстовая система недоступна) —
/// оценка сверху для клика: ~0.6em у моноширинного Menlo (D9.2).
pub(crate) const FALLBACK_CHAR_ADVANCE: f32 = FONT_SIZE * 0.6;

impl Workspace {
    // ---- Координаты мыши → позиция в документе (4.2, D9.2) ----

    /// Bounds uniform_list в координатах окна (после первой отрисовки).
    pub(crate) fn editor_bounds(&self) -> Option<gpui::Bounds<gpui::Pixels>> {
        let b = self.scroll.0.borrow().base_handle.bounds();
        (b.size.width > gpui::Pixels::ZERO && b.size.height > gpui::Pixels::ZERO).then_some(b)
    }

    /// Ширина char: измеряется один раз — advance «0» моноширинного
    /// Menlo через `text_system().ch_advance()`; иначе fallback.
    pub(crate) fn char_advance(&mut self, window: &Window) -> f32 {
        if let Some(a) = self.char_advance {
            return a;
        }
        let advance = measure_char_advance(window);
        self.char_advance = Some(advance);
        advance
    }

    /// Видимый индекс строки по y окна (clamp в список); real-строка
    /// через мапу (в 4.2 — тождественная, folding — 4.5).
    pub(crate) fn visible_line_at_y(&self, y: f32) -> Option<usize> {
        let b = self.editor_bounds()?;
        let offset = f32::from(self.scroll.0.borrow().base_handle.offset().y);
        let vis = (((y - f32::from(b.origin.y) - offset) / LINE_HEIGHT).floor() as i64).max(0)
            as usize;
        Some(vis.min(self.visible_len().saturating_sub(1)))
    }

    /// Колонка по x окна: (x − гуттер) / char_advance, округление к
    /// ближайшей границе символа, clamp по длине строки делает модель.
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
        ((h / LINE_HEIGHT).floor() as usize).max(1)
    }
}

/// Видимость липкой шапки CSV (D12.5): строка 0 полностью ушла из
/// вида — `|offset.y| ≥ LINE_HEIGHT` (offset.y ОТРИЦАТЕЛЕН при
/// прокрутке вниз — см. `visible_line_at_y`; положительный —
/// овёрскролл-отскок вверх, строка 0 видна). Частичный показ
/// (0 < |offset| < LINE_HEIGHT) шапку НЕ дублирует — дубль поверх
/// наполовину видимой строки 0 мерцает.
pub(crate) fn sticky_header_visible(offset_y: f32) -> bool {
    offset_y.abs() >= LINE_HEIGHT
}

/// Измерение advance «0» моноширинного шрифта; fallback 0.6em, если
/// текстовая система недоступна (D9.2).
fn measure_char_advance(window: &Window) -> f32 {
    let ts = window.text_system();
    ts.ch_advance(ts.resolve_font(&gpui::font(FONT)), px(FONT_SIZE))
        .map(f32::from)
        .ok()
        .filter(|a| *a > 0.0)
        .unwrap_or(FALLBACK_CHAR_ADVANCE)
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
            "овёрскролл-отскок вверх — строка 0 видна, шапки нет"
        );
    }
}
