//! Общая клавиатурная логика overlay-диалогов ввода (паттерн
//! goto-диалога D9.2): goto (цифры), выбор языка (фильтр), имена
//! дерева (без '/'). Каждый диалог держит свой `Option<String>`-буфер
//! и фокус; ЭТА часть — единый разбор клавиш: печатаемые символы
//! (фильтр `allowed`, лимит длины в char), backspace, Enter, Escape.
//! Побочные эффекты (закрыть/применить/вернуть фокус) остаются у
//! владельца буфера — `InputKey` говорит, что произошло.

use gpui::KeyDownEvent;

/// Исход клавиши для буфера диалога.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputKey {
    /// Enter — применить (бакфер буфера у владельца).
    Submit,
    /// Escape — отменить.
    Cancel,
    /// Буфер отредактирован (или клавиша проигнорирована).
    Edited,
}

/// Разобрать клавишу в буфер: backspace — удалить последний символ,
/// печатаемые — добавить (фильтр + лимит одновременно в char; keypad
/// без key_char даёт имя клавиши — goto-диалог исторически принимал
/// его, fallback сохранён). Enter/Escape НЕ трогают буфер.
pub(crate) fn feed(
    input: &mut String,
    event: &KeyDownEvent,
    max_chars: usize,
    allowed: impl Fn(char) -> bool,
) -> InputKey {
    let ks = &event.keystroke;
    match ks.key.as_str() {
        "enter" => InputKey::Submit,
        "escape" => InputKey::Cancel,
        "backspace" => {
            input.pop();
            InputKey::Edited
        }
        _ => {
            let chars = ks
                .key_char
                .as_deref()
                .unwrap_or(ks.key.as_str());
            for c in chars.chars().filter(|c| allowed(*c) && !c.is_control()) {
                if input.chars().count() < max_chars {
                    input.push(c);
                }
            }
            InputKey::Edited
        }
    }
}

#[cfg(test)]
mod tests {
    //! Чистая логика буфера: фильтры, лимит, backspace (сами диалоги —
    //! GUI-тесты goto/language/tree).

    use super::*;
    use gpui::{KeyDownEvent, Keystroke, Modifiers};

    fn key(name: &str, ch: Option<&str>) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers::default(),
                key: name.to_string(),
                key_char: ch.map(str::to_string),
            },
            is_held: false,
        }
    }

    #[test]
    fn digits_only_with_limit() {
        let mut input = String::new();
        assert_eq!(feed(&mut input, &key("5", Some("5")), 9, |c| c.is_ascii_digit()), InputKey::Edited);
        assert_eq!(feed(&mut input, &key("a", Some("a")), 9, |c| c.is_ascii_digit()), InputKey::Edited);
        assert_eq!(input, "5");
        for d in 0..12 {
            feed(&mut input, &key("1", Some("1")), 9, |c| c.is_ascii_digit());
            let _ = d;
        }
        assert_eq!(input.chars().count(), 9, "лимит 9 цифр");
    }

    #[test]
    fn tree_name_filters_slash_and_control() {
        let mut input = String::from("ab");
        feed(&mut input, &key("slash", Some("/")), 255, |c| c != '/' );
        feed(&mut input, &key("c", Some("c")), 255, |c| c != '/');
        assert_eq!(input, "abc");
    }

    #[test]
    fn backspace_enter_escape() {
        let mut input = String::from("12");
        assert_eq!(feed(&mut input, &key("backspace", None), 9, |_| true), InputKey::Edited);
        assert_eq!(input, "1");
        assert_eq!(feed(&mut input, &key("enter", None), 9, |_| true), InputKey::Submit);
        assert_eq!(feed(&mut input, &key("escape", None), 9, |_| true), InputKey::Cancel);
        assert_eq!(input, "1", "enter/escape буфер не трогают");
    }

    #[test]
    fn keypad_fallback_uses_key_name() {
        // key_char = None (модификаторы), имя клавиши «0» — цифра
        // проходит (исторический fallback goto-диалога).
        let mut input = String::new();
        feed(&mut input, &key("0", None), 9, |c| c.is_ascii_digit());
        assert_eq!(input, "0");
    }
}
