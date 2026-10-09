//! Тулбар (change toolbar-and-shortcuts): полоса кнопок над всем
//! контентом — группы Файл/Правка/Поиск/Навигация/Вид + дропдауны
//! «Текст»/«Тема» + «?». Кнопки диспатчат те же `Action`-типы, что
//! меню и горячие клавиши (никакой дублирующей логики), тултипы
//! собираются из меток KEYMAP (design D3). Иконки — монохромные SVG
//! (icons.rs), тонированные темой. Переполнение на узких окнах —
//! горизонтальная прокрутка полосы (D8). Открытый дропдаун один,
//! позиционируется `gpui::anchored` от точки клика (D4).

use gpui::{App, MouseDownEvent, MouseButton, Point, Window, anchored, div, prelude::*, px, rgb};

use crate::actions::*;
use crate::theme;
use crate::workspace::Workspace;

/// Выпадающая группа тулбара.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DropdownKind {
    /// Операции со строками (10 пунктов меню Text).
    Text,
    /// System/Dark/Light.
    Theme,
}

/// Источник нажатого состояния toggle-кнопки (D5).
#[derive(Clone, Copy)]
enum ToggleSource {
    SyncScroll,
    MarkdownPreview,
}

impl ToggleSource {
    fn is_on(self, ws: &Workspace) -> bool {
        match self {
            ToggleSource::SyncScroll => ws.sync_scroll,
            ToggleSource::MarkdownPreview => ws.markdown_preview.open,
        }
    }
}

/// Диспетч действия (клик кнопки = пункт меню = горячая клавиша).
type Fire = fn(&mut Window, &mut App);

/// Одна кнопка тулбара (декларативная таблица, design D3).
struct ToolDef {
    id: &'static str,
    icon: &'static str,
    /// i18n-ключ названия (тултип).
    label: &'static str,
    /// Сочетание для тултипа (из KEYMAP).
    key: Option<&'static str>,
    kind: ToolKind,
    /// Разделитель группы перед кнопкой.
    group_start: bool,
}

#[derive(Clone, Copy)]
enum ToolKind {
    Fire(Fire),
    Dropdown(DropdownKind),
    Toggle(Fire, ToggleSource),
}

macro_rules! fire {
    ($name:ident, $action:ident) => {
        fn $name(window: &mut Window, cx: &mut App) {
            window.dispatch_action(Box::new($action), cx);
        }
    };
}

fire!(fire_new_file, NewFile);
fire!(fire_open, OpenFile);
fire!(fire_save, SaveFile);
fire!(fire_undo, Undo);
fire!(fire_redo, Redo);
fire!(fire_find, FindInFile);
fire!(fire_find_tabs, FindInTabs);
fire!(fire_find_folder, FindInFolder);
fire!(fire_goto, GoToLine);
fire!(fire_bm_toggle, ToggleBookmark);
fire!(fire_bm_next, NextBookmark);
fire!(fire_bm_prev, PrevBookmark);
fire!(fire_split, SplitEditor);
fire!(fire_close_pane, ClosePane);
fire!(fire_sync_scroll, ToggleSyncScroll);
fire!(fire_preview, ToggleMarkdownPreview);
fire!(fire_shortcuts, ShowShortcuts);
fire!(fire_sort_asc, SortAsc);
fire!(fire_sort_desc, SortDesc);
fire!(fire_sort_asc_ic, SortAscIgnoreCase);
fire!(fire_sort_desc_ic, SortDescIgnoreCase);
fire!(fire_dedupe, RemoveDuplicateLines);
fire!(fire_remove_empty, RemoveEmptyLines);
fire!(fire_upper, UpperCase);
fire!(fire_lower, LowerCase);
fire!(fire_title, TitleCase);
fire!(fire_sentence, SentenceCase);
fire!(fire_json_pretty, FormatJson);
fire!(fire_json_minify, MinifyJson);
fire!(fire_theme_system, ThemeSystem);
fire!(fire_theme_dark, ThemeDark);
fire!(fire_theme_light, ThemeLight);

/// Полный состав тулбара (слева направо; группы — разделителями).
const TOOLBAR: &[ToolDef] = &[
    // Файл
    ToolDef {
        id: "new",
        icon: "icons/file-plus.svg",
        label: "menu.file.new_file",
        key: Some("cmd-n"),
        kind: ToolKind::Fire(fire_new_file),
        group_start: false,
    },
    ToolDef {
        id: "open",
        icon: "icons/folder-open.svg",
        label: "shortcuts.action.open",
        key: Some("cmd-o"),
        kind: ToolKind::Fire(fire_open),
        group_start: false,
    },
    ToolDef {
        id: "save",
        icon: "icons/save.svg",
        label: "shortcuts.action.save",
        key: Some("cmd-s"),
        kind: ToolKind::Fire(fire_save),
        group_start: false,
    },
    // Правка
    ToolDef {
        id: "undo",
        icon: "icons/undo.svg",
        label: "shortcuts.action.undo",
        key: Some("cmd-z"),
        kind: ToolKind::Fire(fire_undo),
        group_start: true,
    },
    ToolDef {
        id: "redo",
        icon: "icons/redo.svg",
        label: "shortcuts.action.redo",
        key: Some("cmd-shift-z"),
        kind: ToolKind::Fire(fire_redo),
        group_start: false,
    },
    // Поиск
    ToolDef {
        id: "find",
        icon: "icons/search.svg",
        label: "shortcuts.action.find",
        key: Some("cmd-f"),
        kind: ToolKind::Fire(fire_find),
        group_start: true,
    },
    ToolDef {
        id: "find-tabs",
        icon: "icons/layers.svg",
        label: "shortcuts.action.find_in_tabs",
        key: Some("cmd-shift-f"),
        kind: ToolKind::Fire(fire_find_tabs),
        group_start: false,
    },
    ToolDef {
        id: "find-folder",
        icon: "icons/folder-search.svg",
        label: "shortcuts.action.find_in_folder",
        key: Some("cmd-alt-f"),
        kind: ToolKind::Fire(fire_find_folder),
        group_start: false,
    },
    // Навигация
    ToolDef {
        id: "goto",
        icon: "icons/goto-line.svg",
        label: "shortcuts.action.go_to_line",
        key: Some("cmd-g"),
        kind: ToolKind::Fire(fire_goto),
        group_start: true,
    },
    ToolDef {
        id: "bm",
        icon: "icons/bookmark.svg",
        label: "shortcuts.action.toggle_bookmark",
        key: Some("cmd-f2"),
        kind: ToolKind::Fire(fire_bm_toggle),
        group_start: false,
    },
    ToolDef {
        id: "bm-next",
        icon: "icons/chevron-down.svg",
        label: "shortcuts.action.next_bookmark",
        key: Some("f2"),
        kind: ToolKind::Fire(fire_bm_next),
        group_start: false,
    },
    ToolDef {
        id: "bm-prev",
        icon: "icons/chevron-up.svg",
        label: "shortcuts.action.prev_bookmark",
        key: Some("shift-f2"),
        kind: ToolKind::Fire(fire_bm_prev),
        group_start: false,
    },
    // Вид
    ToolDef {
        id: "split",
        icon: "icons/columns-2.svg",
        label: "shortcuts.action.split_editor",
        key: Some("cmd-\\"),
        kind: ToolKind::Fire(fire_split),
        group_start: true,
    },
    ToolDef {
        id: "close-pane",
        icon: "icons/x.svg",
        label: "shortcuts.action.close_pane",
        key: Some("cmd-shift-\\"),
        kind: ToolKind::Fire(fire_close_pane),
        group_start: false,
    },
    ToolDef {
        id: "sync",
        icon: "icons/arrow-down-up.svg",
        label: "menu.view.sync_scroll",
        key: None,
        kind: ToolKind::Toggle(fire_sync_scroll, ToggleSource::SyncScroll),
        group_start: false,
    },
    ToolDef {
        id: "preview",
        icon: "icons/eye.svg",
        label: "shortcuts.action.markdown_preview",
        key: Some("cmd-shift-v"),
        kind: ToolKind::Toggle(fire_preview, ToggleSource::MarkdownPreview),
        group_start: false,
    },
    // Группы-дропдауны
    ToolDef {
        id: "text-ops",
        icon: "icons/type.svg",
        label: "toolbar.text",
        key: None,
        kind: ToolKind::Dropdown(DropdownKind::Text),
        group_start: true,
    },
    ToolDef {
        id: "theme",
        icon: "icons/palette.svg",
        label: "toolbar.theme",
        key: None,
        kind: ToolKind::Dropdown(DropdownKind::Theme),
        group_start: false,
    },
    // Справка
    ToolDef {
        id: "help",
        icon: "icons/circle-help.svg",
        label: "menu.help.shortcuts",
        key: Some("cmd-/"),
        kind: ToolKind::Fire(fire_shortcuts),
        group_start: true,
    },
];

/// Высота тулбара (фиксированная — layout ниже не дёргается, спека).
const TOOLBAR_H: f32 = 36.0;

impl Workspace {
    /// Тумблер View → Toolbar: виден/скрыт, состояние — в сессии.
    pub(crate) fn on_toggle_toolbar(
        &mut self,
        _: &ToggleToolbar,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.toolbar_visible = !self.toolbar_visible;
        cx.notify();
    }

    /// Полоса тулбара: группы кнопок с h-scroll на узких окнах (спека
    /// «Переполнение»), тултип — anchored у курсора (D3).
    pub(crate) fn render_toolbar(&self, cx: &gpui::Context<Self>) -> gpui::AnyElement {
        let theme = theme::current();
        let mut bar = div()
            .id("toolbar")
            .debug_selector(|| "toolbar".to_string())
            .flex()
            .items_center()
            .flex_shrink_0()
            .h(px(TOOLBAR_H))
            .w_full()
            .px_1()
            .bg(rgb(theme.tabbar))
            .border_b_1()
            .border_color(rgb(theme.close_hover))
            .overflow_x_scroll();
        for def in TOOLBAR {
            if def.group_start {
                bar = bar.child(
                    div()
                        .w(px(1.0))
                        .h(px(20.0))
                        .flex_shrink_0()
                        .mx_1()
                        .bg(rgb(theme.close_hover)),
                );
            }
            bar = bar.child(self.render_tool_button(def, cx));
        }
        // Тултип наведённой кнопки (anchored к позиции курсора).
        if let Some((id, pos)) = self.toolbar_hover
            && let Some(def) = TOOLBAR.iter().find(|d| d.id == id)
        {
            bar = bar.child(self.render_tool_tooltip(def, pos, cx));
        }
        bar.into_any_element()
    }

    /// Одна кнопка: иконка 16×16, hover-фон, нажатое состояние toggle.
    fn render_tool_button(&self, def: &ToolDef, cx: &gpui::Context<Self>) -> gpui::AnyElement {
        let theme = theme::current();
        let pressed = match &def.kind {
            ToolKind::Toggle(_, src) => src.is_on(self),
            _ => false,
        };
        let hovered = self.toolbar_hover.is_some_and(|(id, _)| id == def.id);
        let (fg, bg) = match (pressed, hovered) {
            (true, _) => (theme.accent, theme.close_hover),
            (false, true) => (theme.text, theme.close_hover),
            (false, false) => (theme.tab_inactive, 0),
        };
        let id = def.id;
        let kind = def.kind; // Copy: 'static для listener-замыканий
        div()
            .id(def.id)
            .debug_selector(move || format!("tool-{id}"))
            .flex()
            .items_center()
            .justify_center()
            .w(px(28.0))
            .h(px(28.0))
            .mx_0p5()
            .flex_shrink_0()
            .rounded_md()
            .when(bg != 0, |b| b.bg(rgb(bg)))
            .when(pressed, |b| b.border_1().border_color(rgb(theme.accent)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.toolbar_hover = None;
                    match kind {
                        ToolKind::Fire(f) | ToolKind::Toggle(f, _) => f(window, &mut *cx),
                        ToolKind::Dropdown(kind) => {
                            let pos = event.position;
                            this.open_toolbar_dropdown(kind, pos, cx);
                            let f = this.toolbar_dd_focus.clone();
                            window.focus(&f);
                        }
                    }
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &gpui::MouseMoveEvent, _window, cx| {
                    if !this.toolbar_hover.as_ref().is_some_and(|(h, _)| *h == id) {
                        let pos = event.position;
                        this.toolbar_hover = Some((id, pos));
                        cx.notify();
                    }
                }),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                if !hovered && this.toolbar_hover.is_some_and(|(h, _)| h == id) {
                    this.toolbar_hover = None;
                    cx.notify();
                }
            }))
            .child(
                gpui::svg()
                    .path(def.icon)
                    .w_4()
                    .h_4()
                    .text_color(rgb(fg)),
            )
            .into_any_element()
    }

    /// Тултип: название + клавиша (i18n; anchored у курсора).
    fn render_tool_tooltip(
        &self,
        def: &ToolDef,
        pos: Point<gpui::Pixels>,
        cx: &gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let theme = theme::current();
        let label = crate::i18n::t(cx, def.label);
        let text = match def.key {
            Some(key) => format!("{label} ({})", crate::keymap::pretty_key(key)),
            None => label,
        };
        anchored()
            .position(pos)
            .anchor(gpui::Corner::BottomLeft)
            .child(
                div()
                    .id("tool-tooltip")
                    .debug_selector(|| "tool-tooltip".to_string())
                    .px_2()
                    .py_1()
                    .mt_1()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text))
                    .bg(rgb(theme.panel))
                    .border_1()
                    .border_color(rgb(theme.close_hover))
                    .rounded_md()
                    .shadow_md()
                    .child(text),
            )
            .into_any_element()
    }

    /// Открыть дропдаун у позиции клика (один активный).
    pub(crate) fn open_toolbar_dropdown(
        &mut self,
        kind: DropdownKind,
        pos: Point<gpui::Pixels>,
        cx: &mut gpui::Context<Self>,
    ) {
        // Повторный клик по открытой группе — закрыть (тумблер).
        if self.toolbar_dropdown.as_ref().is_some_and(|k| *k == kind) {
            self.toolbar_dropdown = None;
        } else {
            self.toolbar_dropdown = Some(kind);
        }
        self.toolbar_dropdown_pos = Some(pos);
        cx.notify();
    }

    /// Дропдаун тулбара: подложка (клик-вне закрывает) + меню у точки
    /// клика (спека «Выпадающие группы»).
    pub(crate) fn render_toolbar_dropdown(&self, cx: &gpui::Context<Self>) -> gpui::AnyElement {
        let theme = theme::current();
        let Some(kind) = self.toolbar_dropdown else {
            return div().into_any_element();
        };
        let Some(pos) = self.toolbar_dropdown_pos else {
            return div().into_any_element();
        };
        struct Item {
            label_key: &'static str,
            fire: Option<Fire>,
            checked: bool,
        }
        let items: Vec<Item> = match kind {
            DropdownKind::Text => [
                ("menu.text.sort_asc", fire_sort_asc as Fire),
                ("menu.text.sort_desc", fire_sort_desc),
                ("menu.text.sort_asc_ic", fire_sort_asc_ic),
                ("menu.text.sort_desc_ic", fire_sort_desc_ic),
                ("menu.text.dedupe", fire_dedupe),
                ("menu.text.remove_empty", fire_remove_empty),
                ("menu.text.upper", fire_upper),
                ("menu.text.lower", fire_lower),
                ("menu.text.title", fire_title),
                ("menu.text.sentence", fire_sentence),
                ("menu.text.json_pretty", fire_json_pretty),
                ("menu.text.json_minify", fire_json_minify),
            ]
            .into_iter()
            .map(|(label_key, fire)| Item { label_key, fire: Some(fire), checked: false })
            .collect(),
            DropdownKind::Theme => {
                let active = self.theme_kind;
                [
                    ("menu.view.theme.system", fire_theme_system as Fire, active == crate::theme::ThemeKind::System),
                    ("menu.view.theme.dark", fire_theme_dark, active == crate::theme::ThemeKind::Dark),
                    ("menu.view.theme.light", fire_theme_light, active == crate::theme::ThemeKind::Light),
                ]
                .into_iter()
                .map(|(label_key, fire, checked)| Item { label_key, fire: Some(fire), checked })
                .collect()
            }
        };
        let mut list = div().flex().flex_col().p_1();
        for item in &items {
            let fire = item.fire;
            let label_key = item.label_key;
            let label = crate::i18n::t(cx, item.label_key);
            let checked = item.checked;
            list = list.child(
                div()
                    .id(item.label_key)
                    .debug_selector(move || format!("dd-item-{label_key}"))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .min_w(px(200.0))
                    .rounded_sm()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text))
                    .hover(|s| s.bg(rgb(theme.close_hover)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _e, window, cx| {
                            this.toolbar_dropdown = None;
                            if let Some(fire) = fire {
                                fire(window, &mut *cx);
                            }
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .w(px(12.0))
                            .text_color(rgb(theme.accent))
                            .child(if checked { "✓" } else { "" }),
                    )
                    .child(label),
            );
        }
        div()
            .id("dd-underlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _e, _w, cx| {
                    this.toolbar_dropdown = None;
                    cx.notify();
                }),
            )
            .child(
                anchored()
                    .position(pos)
                    .anchor(gpui::Corner::TopLeft)
                    .child(
                        div()
                            .id("dd-menu")
                            .debug_selector(|| "toolbar-dropdown".to_string())
                            .flex()
                            .flex_col()
                            .bg(rgb(theme.panel))
                            .border_1()
                            .border_color(rgb(theme.close_hover))
                            .rounded_md()
                            .shadow_md()
                            .track_focus(&self.toolbar_dd_focus)
                            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                                if event.keystroke.key == "escape" {
                                    this.toolbar_dropdown = None;
                                    window.focus(&this.active_focus());
                                    cx.notify();
                                }
                            }))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(list),
                    ),
            )
            .into_any_element()
    }
}

