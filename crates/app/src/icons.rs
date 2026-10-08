//! Встраиваемые SVG-ассеты (toolbar-and-shortcuts D2): монохромные
//! иконки (штрих, `currentColor`) вшиты в бинарник через `include_str!`
//! и раздаются кастомным `AssetSource` — никаких внешних файлов и
//! зависимостей. gpui рендерит SVG как `MonochromeSprite` и тонирует
//! цветом `text_color` элемента: перекраска темой гарантирована самим
//! движком (design D2, риска «currentColor» нет).

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

/// Карта «путь → SVG» (все ассеты приложения).
const ICONS: &[(&str, &str)] = &[
    ("icons/file-plus.svg", include_str!("../assets/icons/file-plus.svg")),
    ("icons/folder-open.svg", include_str!("../assets/icons/folder-open.svg")),
    ("icons/save.svg", include_str!("../assets/icons/save.svg")),
    ("icons/undo.svg", include_str!("../assets/icons/undo.svg")),
    ("icons/redo.svg", include_str!("../assets/icons/redo.svg")),
    ("icons/search.svg", include_str!("../assets/icons/search.svg")),
    ("icons/layers.svg", include_str!("../assets/icons/layers.svg")),
    ("icons/folder-search.svg", include_str!("../assets/icons/folder-search.svg")),
    ("icons/goto-line.svg", include_str!("../assets/icons/goto-line.svg")),
    ("icons/bookmark.svg", include_str!("../assets/icons/bookmark.svg")),
    ("icons/chevron-down.svg", include_str!("../assets/icons/chevron-down.svg")),
    ("icons/chevron-up.svg", include_str!("../assets/icons/chevron-up.svg")),
    ("icons/columns-2.svg", include_str!("../assets/icons/columns-2.svg")),
    ("icons/x.svg", include_str!("../assets/icons/x.svg")),
    ("icons/arrow-down-up.svg", include_str!("../assets/icons/arrow-down-up.svg")),
    ("icons/eye.svg", include_str!("../assets/icons/eye.svg")),
    ("icons/type.svg", include_str!("../assets/icons/type.svg")),
    ("icons/palette.svg", include_str!("../assets/icons/palette.svg")),
    ("icons/circle-help.svg", include_str!("../assets/icons/circle-help.svg")),
];

/// Asset-источник приложения (встроенные ресурсы).
pub(crate) struct EmbeddedAssets;

impl AssetSource for EmbeddedAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, svg)| Cow::Owned(svg.as_bytes().to_vec())))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS.iter().map(|(p, _)| SharedString::from(*p)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2.1: все иконки — валидный XML с viewBox 24 (движок gpui парсит
    /// их при первом рендере; сломанный файл = пустая кнопка).
    #[test]
    fn icons_are_valid_svg() {
        for (path, svg) in ICONS {
            assert!(svg.contains("<svg"), "{path}: нет svg-корня");
            assert!(svg.contains("viewBox=\"0 0 24 24\""), "{path}: не 24×24");
            assert!(svg.contains("</svg>"), "{path}: не закрыт");
        }
    }

    /// 2.1: пути уникальны, загрузчик находит каждый.
    #[test]
    fn assets_load_by_path() {
        for (path, _) in ICONS {
            let loaded = EmbeddedAssets.load(path).unwrap();
            assert!(loaded.is_some(), "{path} не найден");
        }
        assert!(EmbeddedAssets.load("icons/no-such.svg").unwrap().is_none());
        assert_eq!(EmbeddedAssets.list("").unwrap().len(), ICONS.len());
    }
}
