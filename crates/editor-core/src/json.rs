//! Форматирование JSON (поддержка JSON): читаемый вид (pretty,
//! 2 пробела) и компактный стиль (minify). Разбор — serde_json
//! (строгий RFC 8259); ошибка возвращает позицию line/column для
//! статус-бара. Модуль GPUI-free: строка на входе — строка на выходе.

use serde_json::Value;

/// Стиль вывода форматтера.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonStyle {
    /// Читаемый вид: отступы 2 пробела, ключ: значение.
    Pretty,
    /// Компактный стиль: без пробелов и переводов строк.
    Compact,
}

/// Ошибка разбора JSON: сообщение serde_json + позиция (1-based).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}, column {}: {}", self.line, self.col, self.message)
    }
}

/// Отформатировать JSON в заданном стиле. Перевод строки в конце
/// сохраняется (файл с финальным '\n' остаётся с ним). Пробельные
/// изменения при равном выводе — не ошибка: строка возвращается как есть.
pub fn format_json(src: &str, style: JsonStyle) -> Result<String, JsonError> {
    let value: Value = serde_json::from_str(src).map_err(|e| JsonError {
        line: e.line(),
        col: e.column(),
        message: e.to_string(),
    })?;
    let had_trailing_newline = src.ends_with('\n');
    let mut out = match style {
        JsonStyle::Pretty => serde_json::to_string_pretty(&value),
        JsonStyle::Compact => serde_json::to_string(&value),
    }
    .map_err(|e| JsonError { line: 0, col: 0, message: e.to_string() })?;
    if had_trailing_newline {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRETTY: &str = "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    null\n  ]\n}";

    #[test]
    fn pretty_prints_minified_input() {
        let out = format_json("{\"a\":1,\"b\":[true,null]}", JsonStyle::Pretty).unwrap();
        assert_eq!(out, PRETTY);
    }

    #[test]
    fn compact_minifies_pretty_input() {
        let out = format_json(PRETTY, JsonStyle::Compact).unwrap();
        assert_eq!(out, "{\"a\":1,\"b\":[true,null]}");
    }

    #[test]
    fn trailing_newline_is_preserved() {
        let out = format_json("{\"a\":1}\n", JsonStyle::Pretty).unwrap();
        assert_eq!(out, "{\n  \"a\": 1\n}\n");
        let out = format_json("{\n  \"a\": 1\n}\n", JsonStyle::Compact).unwrap();
        assert_eq!(out, "{\"a\":1}\n");
    }

    #[test]
    fn error_carries_position() {
        let err = format_json("{\"a\": }", JsonStyle::Pretty).unwrap_err();
        assert_eq!(err.line, 1);
        assert_eq!(err.col, 7);
        assert!(!err.message.is_empty());
    }

    #[test]
    fn unicode_and_escapes_roundtrip() {
        let src = "{\"текст\":\"значение \\\"в кавычках\\\"\"}";
        let pretty = format_json(src, JsonStyle::Pretty).unwrap();
        assert_eq!(format_json(&pretty, JsonStyle::Compact).unwrap(), src);
    }

    #[test]
    fn idempotent_on_both_styles() {
        let src = "{\"x\":[1,{\"y\":\"z\"}],\"w\":false}";
        let pretty = format_json(src, JsonStyle::Pretty).unwrap();
        assert_eq!(format_json(&pretty, JsonStyle::Pretty).unwrap(), pretty);
        let compact = format_json(src, JsonStyle::Compact).unwrap();
        assert_eq!(format_json(&compact, JsonStyle::Compact).unwrap(), compact);
    }
}
