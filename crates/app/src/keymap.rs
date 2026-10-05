//! Горячие клавиши (8.5/D15.5): единый источник `KEYMAP` — и для
//! `bind_keys` (main), и для справочника сочетаний (оверлей Cmd+/).
//! Стандартные сочетания (Cmd+S, Cmd+F …) — общепринятые; macOS-стиль.
//!
//! Справочник: Help → Keyboard Shortcuts (Cmd+/) — секции из
//! `KEYMAP`, подписи действий — i18n-ключи.

use gpui::{App, KeyBinding};

use crate::actions::*;

/// Действие раскладки (для биндинга и справочника).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyAction {
    Open,
    OpenWorkspace,
    Save,
    SaveAs,
    CloseTab,
    NewTab,
    NextTab,
    PrevTab,
    Undo,
    Redo,
    GoToLine,
    AddCursor,
    ToggleBookmark,
    NextBookmark,
    PrevBookmark,
    Find,
    FindInTabs,
    FindInFolder,
    SplitEditor,
    ClosePane,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ShowShortcuts,
    CycleLanguage,
    NewFile,
    NewFolder,
    RenameNode,
    DeleteNode,
    FoldAll,
    UnfoldAll,
}

/// Одна запись раскладки: сочетание + действие + секция/подпись для
/// справочника (i18n-ключи).
pub(crate) struct KeyDef {
    pub key: &'static str,
    pub action: KeyAction,
    /// Секция справочника (i18n-ключ shortcuts.section.*).
    pub section: &'static str,
    /// Подпись действия (i18n-ключ shortcuts.action.*).
    pub label: &'static str,
}

/// Полная раскладка (8.5): открытие/сохранение, вкладки, правка,
/// навигация, поиск, вид (split/шрифт), язык интерфейса, дерево.
pub(crate) const KEYMAP: &[KeyDef] = &[
    // Файлы
    KeyDef { key: "cmd-o", action: KeyAction::Open, section: "file", label: "shortcuts.action.open" },
    KeyDef { key: "cmd-shift-o", action: KeyAction::OpenWorkspace, section: "file", label: "shortcuts.action.open_workspace" },
    KeyDef { key: "cmd-s", action: KeyAction::Save, section: "file", label: "shortcuts.action.save" },
    KeyDef { key: "cmd-shift-s", action: KeyAction::SaveAs, section: "file", label: "shortcuts.action.save_as" },
    // Вкладки
    KeyDef { key: "cmd-t", action: KeyAction::NewTab, section: "tabs", label: "shortcuts.action.new_tab" },
    KeyDef { key: "cmd-w", action: KeyAction::CloseTab, section: "tabs", label: "shortcuts.action.close_tab" },
    KeyDef { key: "cmd-shift-]", action: KeyAction::NextTab, section: "tabs", label: "shortcuts.action.next_tab" },
    KeyDef { key: "cmd-shift-[", action: KeyAction::PrevTab, section: "tabs", label: "shortcuts.action.prev_tab" },
    // Правка
    KeyDef { key: "cmd-z", action: KeyAction::Undo, section: "edit", label: "shortcuts.action.undo" },
    KeyDef { key: "cmd-shift-z", action: KeyAction::Redo, section: "edit", label: "shortcuts.action.redo" },
    // Навигация
    KeyDef { key: "cmd-g", action: KeyAction::GoToLine, section: "navigation", label: "shortcuts.action.go_to_line" },
    KeyDef { key: "cmd-d", action: KeyAction::AddCursor, section: "navigation", label: "shortcuts.action.add_cursor" },
    KeyDef { key: "cmd-f2", action: KeyAction::ToggleBookmark, section: "navigation", label: "shortcuts.action.toggle_bookmark" },
    KeyDef { key: "f2", action: KeyAction::NextBookmark, section: "navigation", label: "shortcuts.action.next_bookmark" },
    KeyDef { key: "shift-f2", action: KeyAction::PrevBookmark, section: "navigation", label: "shortcuts.action.prev_bookmark" },
    // Поиск
    KeyDef { key: "cmd-f", action: KeyAction::Find, section: "search", label: "shortcuts.action.find" },
    KeyDef { key: "cmd-shift-f", action: KeyAction::FindInTabs, section: "search", label: "shortcuts.action.find_in_tabs" },
    KeyDef { key: "cmd-alt-f", action: KeyAction::FindInFolder, section: "search", label: "shortcuts.action.find_in_folder" },
    // Вид: split + шрифт + справочник
    KeyDef { key: "cmd-\\", action: KeyAction::SplitEditor, section: "view", label: "shortcuts.action.split_editor" },
    KeyDef { key: "cmd-shift-\\", action: KeyAction::ClosePane, section: "view", label: "shortcuts.action.close_pane" },
    KeyDef { key: "cmd-=", action: KeyAction::ZoomIn, section: "view", label: "shortcuts.action.zoom_in" },
    KeyDef { key: "cmd--", action: KeyAction::ZoomOut, section: "view", label: "shortcuts.action.zoom_out" },
    KeyDef { key: "cmd-0", action: KeyAction::ZoomReset, section: "view", label: "shortcuts.action.zoom_reset" },
    KeyDef { key: "cmd-/", action: KeyAction::ShowShortcuts, section: "view", label: "shortcuts.action.shortcuts" },
    KeyDef { key: "cmd-shift-l", action: KeyAction::CycleLanguage, section: "view", label: "shortcuts.action.interface_language" },
    // Дерево воркспейса
    KeyDef { key: "cmd-n", action: KeyAction::NewFile, section: "file", label: "shortcuts.action.tree_new_file" },
    KeyDef { key: "cmd-shift-n", action: KeyAction::NewFolder, section: "file", label: "shortcuts.action.tree_new_folder" },
    KeyDef { key: "shift-f6", action: KeyAction::RenameNode, section: "file", label: "shortcuts.action.tree_rename" },
    KeyDef { key: "cmd-backspace", action: KeyAction::DeleteNode, section: "file", label: "shortcuts.action.tree_delete" },
    // Свёртывание
    KeyDef { key: "cmd-alt-[", action: KeyAction::FoldAll, section: "view", label: "menu.view.fold_all" },
    KeyDef { key: "cmd-alt-]", action: KeyAction::UnfoldAll, section: "view", label: "menu.view.unfold_all" },
];

/// Забиндить всю раскладку (main при старте; KEYMAP — единственный
/// источник: биндинг и справочник не расходятся).
pub(crate) fn bind_all(cx: &mut App) {
    let mut bindings = Vec::new();
    for def in KEYMAP {
        let binding = match def.action {
            KeyAction::Open => KeyBinding::new(def.key, OpenFile, None),
            KeyAction::OpenWorkspace => KeyBinding::new(def.key, OpenWorkspace, None),
            KeyAction::Save => KeyBinding::new(def.key, SaveFile, None),
            KeyAction::SaveAs => KeyBinding::new(def.key, SaveFileAs, None),
            KeyAction::CloseTab => KeyBinding::new(def.key, CloseTab, None),
            KeyAction::NewTab => KeyBinding::new(def.key, NewTab, None),
            KeyAction::NextTab => KeyBinding::new(def.key, NextTab, None),
            KeyAction::PrevTab => KeyBinding::new(def.key, PrevTab, None),
            KeyAction::Undo => KeyBinding::new(def.key, Undo, None),
            KeyAction::Redo => KeyBinding::new(def.key, Redo, None),
            KeyAction::GoToLine => KeyBinding::new(def.key, GoToLine, None),
            KeyAction::AddCursor => KeyBinding::new(def.key, AddNextMatch, None),
            KeyAction::ToggleBookmark => KeyBinding::new(def.key, ToggleBookmark, None),
            KeyAction::NextBookmark => KeyBinding::new(def.key, NextBookmark, None),
            KeyAction::PrevBookmark => KeyBinding::new(def.key, PrevBookmark, None),
            KeyAction::Find => KeyBinding::new(def.key, FindInFile, None),
            KeyAction::FindInTabs => KeyBinding::new(def.key, FindInTabs, None),
            KeyAction::FindInFolder => KeyBinding::new(def.key, FindInFolder, None),
            KeyAction::SplitEditor => KeyBinding::new(def.key, SplitEditor, None),
            KeyAction::ClosePane => KeyBinding::new(def.key, ClosePane, None),
            KeyAction::ZoomIn => KeyBinding::new(def.key, ZoomIn, None),
            KeyAction::ZoomOut => KeyBinding::new(def.key, ZoomOut, None),
            KeyAction::ZoomReset => KeyBinding::new(def.key, ZoomReset, None),
            KeyAction::ShowShortcuts => KeyBinding::new(def.key, ShowShortcuts, None),
            KeyAction::CycleLanguage => KeyBinding::new(def.key, CycleLanguage, None),
            KeyAction::NewFile => KeyBinding::new(def.key, NewFile, None),
            KeyAction::NewFolder => KeyBinding::new(def.key, NewFolder, None),
            KeyAction::RenameNode => KeyBinding::new(def.key, RenameNode, None),
            KeyAction::DeleteNode => KeyBinding::new(def.key, DeleteNode, None),
            KeyAction::FoldAll => KeyBinding::new(def.key, FoldAll, None),
            KeyAction::UnfoldAll => KeyBinding::new(def.key, UnfoldAll, None),
        };
        bindings.push(binding);
    }
    cx.bind_keys(bindings);
}

/// Отрисовать сочетание по-человечески: "cmd-shift-]" → "⌘⇧]",
/// "f2" → "F2" (справочник и меню).
pub(crate) fn pretty_key(key: &str) -> String {
    let mut out = String::new();
    for part in key.split('-') {
        match part {
            "cmd" => out.push('⌘'),
            "shift" => out.push('⇧'),
            "alt" => out.push('⌥'),
            "ctrl" => out.push('⌃'),
            other => out.push_str(&other.to_uppercase()),
        }
    }
    out
}

impl crate::workspace::Workspace {
    /// Оверлей-справочник сочетаний (8.5): секции KEYMAP с подписями
    /// i18n; Esc/клик по подложке — закрыть.
    pub(crate) fn render_shortcuts_overlay(
        &self,
        cx: &gpui::Context<Self>,
    ) -> gpui::AnyElement {
        use gpui::prelude::*;
        let t = |k: &str| crate::i18n::t(cx, k);
        let mut list = gpui::div().flex().flex_col();
        // Секции в порядке первого появления (KEYMAP сгруппирован).
        let mut sections: Vec<(&str, Vec<&KeyDef>)> = Vec::new();
        for def in KEYMAP {
            match sections.iter_mut().find(|(s, _)| *s == def.section) {
                Some((_, defs)) => defs.push(def),
                None => sections.push((def.section, vec![def])),
            }
        }
        for (section, defs) in sections {
            list = list.child(
                gpui::div()
                    .mt_2()
                    .px_3()
                    .text_size(gpui::px(11.0))
                    .text_color(gpui::rgb(crate::theme::current().panel_header))
                    .child(t(&format!("shortcuts.section.{section}"))),
            );
            for def in defs {
                list = list.child(
                    gpui::div()
                        .flex()
                        .justify_between()
                        .gap_4()
                        .px_3()
                        .py_0p5()
                        .text_size(gpui::px(12.0))
                        .text_color(gpui::rgb(crate::theme::current().text))
                        .child(gpui::div().child(t(def.label)))
                        .child(
                            gpui::div()
                                .text_color(gpui::rgb(crate::theme::current().dim))
                                .child(pretty_key(def.key)),
                        ),
                );
            }
        }
        gpui::div()
            .id("shortcuts-underlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(gpui::black().opacity(0.2))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _e, _w, cx| {
                    this.shortcuts_open = false;
                    cx.notify();
                }),
            )
            .child(
                gpui::div()
                    .id("shortcuts-overlay")
                    .debug_selector(|| "shortcuts-overlay".to_string())
                    .absolute()
                    .top(gpui::px(60.0))
                    .left(gpui::px(340.0))
                    .w(gpui::px(380.0))
                    .max_h(gpui::px(520.0))
                    .flex()
                    .flex_col()
                    .pb_2()
                    .bg(gpui::rgb(crate::theme::current().panel))
                    .border_1()
                    .border_color(gpui::rgb(crate::theme::current().close_hover))
                    .rounded_md()
                    .shadow_md()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        |_, _, cx| cx.stop_propagation(),
                    )
                    .track_focus(&self.shortcuts_focus)
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _window, cx| {
                        if event.keystroke.key == "escape" {
                            this.shortcuts_open = false;
                            cx.notify();
                        }
                    }))
                    .child(
                        gpui::div()
                            .flex()
                            .justify_between()
                            .px_3()
                            .py_2()
                            .text_size(gpui::px(12.0))
                            .bg(gpui::rgb(crate::theme::current().tabbar))
                            .text_color(gpui::rgb(crate::theme::current().dim))
                            .child(gpui::div().child(t("shortcuts.title")))
                            .child(gpui::div().child(t("shortcuts.hint"))),
                    )
                    .child(
                        list.id("shortcuts-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll(),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 8.5: сочетания уникальны (двойное назначение — баг раскладки).
    #[test]
    fn keymap_keys_unique() {
        let mut keys: Vec<&str> = KEYMAP.iter().map(|d| d.key).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "дубликаты сочетаний: {keys:?}");
    }

    /// 8.5: человекочитаемые сочетания.
    #[test]
    fn pretty_keys() {
        assert_eq!(pretty_key("cmd-o"), "⌘O");
        assert_eq!(pretty_key("cmd-shift-]"), "⌘⇧]");
        assert_eq!(pretty_key("f2"), "F2");
        assert_eq!(pretty_key("shift-f2"), "⇧F2");
        assert_eq!(pretty_key("cmd-alt-f"), "⌘⌥F");
    }
}
