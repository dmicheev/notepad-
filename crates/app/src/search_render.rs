//! Рендер панели поиска (6.1, D13): абсолютный оверлей top-right
//! редактора (чипы/поля/счётчик/навигация) и секция SEARCH левой
//! панели (результаты Tabs/Folder). Логика и состояние —
//! search_panel.rs; фоновые сканы — search_scan.rs.

use gpui::{Context, MouseButton, div, prelude::*, px, rgb};

use crate::search_panel::*;
use crate::search_panel::SEARCH_PANEL_W;
use crate::theme::*;
use crate::workspace::Workspace;

impl Workspace {
    /// Панель поиска: абсолютный оверлей top-right контейнера
    /// редактора (контейнер .relative() — якорь, паттерн липкой шапки
    /// D12.5), постоянная пока открыта. Отдельный фокус — клавиши
    /// панели не доходят до редактора (D9.2). Клики останавливаются
    /// на корне панели (не двигают каретку редактора).
    pub(crate) fn render_search_panel(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.sync_search_matches();
        let sp = self.search.as_ref().expect("render_search_panel with panel");
        let is_file = sp.scope == SearchScope::File;
        let is_folder = matches!(sp.scope, SearchScope::Folder(_));
        let folder_name = match &sp.scope {
            SearchScope::Folder(f) => f
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| f.display().to_string()),
            _ => String::new(),
        };
        let counter = sp.counter();
        let query = sp.query.clone();
        let replacement = sp.replacement.clone();
        let mask = sp.mask.clone();
        let case = sp.case_sensitive;
        let word = sp.whole_word;
        let regex = sp.regex_mode;
        let show_replace = sp.show_replace;
        let field = sp.field;
        let error = sp.error.clone();
        let scanning = sp.scanning;
        let summary = sp.summary;
        let results_len = sp.results.len();
        let truncated = sp.truncated;

        let field_box = |text: String, placeholder: &str, active: bool, id: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e, window, cx| {
                        cx.stop_propagation();
                        let f = match id {
                            "search-field-query" => SearchField::Query,
                            "search-field-replace" => SearchField::Replace,
                            _ => SearchField::Mask,
                        };
                        this.on_search_field(f, e, window, cx);
                    }),
                )
                .flex_1()
                .min_w_0()
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(rgb(BG))
                .border_1()
                .border_color(rgb(if active { ACCENT } else { CLOSE_HOVER }))
                .text_size(px(12.0))
                .text_color(rgb(if text.is_empty() { DIM } else { TEXT }))
                .child(if text.is_empty() { placeholder.to_string() } else { text })
        };

        let chip = |label: &'static str, active: bool, id: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e, window, cx| {
                        cx.stop_propagation();
                        let what: &str = match id {
                            "search-toggle-case" => "case",
                            "search-toggle-word" => "word",
                            "search-toggle-regex" => "regex",
                            "search-toggle-replace" => "replace",
                            _ => "",
                        };
                        if !what.is_empty() {
                            this.on_search_toggle(what, e, window, cx);
                        }
                    }),
                )
                .px_2()
                .py_1()
                .text_size(px(11.0))
                .rounded_sm()
                .when(active, |d| d.bg(rgb(ACCENT)).text_color(rgb(STATUS_FG)))
                .when(!active, |d| d.bg(rgb(CLOSE_HOVER)).text_color(rgb(TAB_INACTIVE)))
                .hover(|d| d.bg(rgb(if active { STATUS_LANG_HOVER } else { CLOSE_HOVER })))
                .child(label)
        };

        // Строка 1: scope-чипы + запрос + счётчик + опции + навигация.
        let scope_chip = |label: &'static str, active: bool, id: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _e, window, cx| {
                        cx.stop_propagation();
                        let scope: &str = match id {
                            "search-scope-file" => "file",
                            "search-scope-tabs" => "tabs",
                            _ => "folder",
                        };
                        this.on_search_scope(scope, window, cx);
                    }),
                )
                .px_2()
                .py_1()
                .text_size(px(11.0))
                .rounded_sm()
                .when(active, |d| d.bg(rgb(ACCENT)).text_color(rgb(STATUS_FG)))
                .when(!active, |d| d.bg(rgb(CLOSE_HOVER)).text_color(rgb(TAB_INACTIVE)))
                .child(label)
        };

        let mut panel = div()
            .id("search-panel")
            .debug_selector(|| "search-panel".to_string())
            .absolute()
            .top_0()
            // Вплотную к вертикальной дорожке (не перекрывает ползунок).
            .right(px(crate::scrollbar::TRACK_W))
            .w(px(SEARCH_PANEL_W))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .bg(rgb(TABBAR))
            .border_1()
            .border_color(rgb(CLOSE_HOVER))
            .rounded_bl_md()
            .shadow_md()
            .track_focus(&self.search_focus)
            .on_key_down(cx.listener(Self::handle_search_key))
            // Клики по панели не двигают каретку редактора (панель —
            // ребёнок контейнера редактора, on_editor_mouse_down).
            .on_mouse_down(MouseButton::Left, cx.listener(|_this, _e, _w, cx| {
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(scope_chip("File", is_file, "search-scope-file"))
                    .child(scope_chip("Tabs", sp.scope == SearchScope::Tabs, "search-scope-tabs"))
                    .child(scope_chip(
                        "Folder",
                        is_folder,
                        "search-scope-folder",
                    ))
                    .child(
                        field_box(
                            query.clone(),
                            if is_file { "Search…" } else { "Search (press Enter)…" },
                            field == SearchField::Query,
                            "search-field-query",
                        )
                        .flex_1(),
                    )
                    // Лупа — запуск (замечание юзабилити): дублирует
                    // Enter — File: следующее совпадение, Tabs/Folder:
                    // поиск. Кликабельна без фокуса в поле.
                    .child(
                        div()
                            .id("search-go")
                            .debug_selector(|| "search-go".to_string())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _e, window, cx| {
                                    cx.stop_propagation();
                                    this.search_enter(window, cx);
                                }),
                            )
                            .px_2()
                            .py_1()
                            .text_size(px(12.0))
                            .rounded_sm()
                            .bg(rgb(CLOSE_HOVER))
                            .text_color(rgb(TEXT))
                            .hover(|d| d.bg(rgb(ACCENT)).text_color(rgb(STATUS_FG)))
                            .child("🔍"),
                    )
                    .child(
                        div()
                            .id("search-counter")
                            .debug_selector(|| "search-counter".to_string())
                            .px_1()
                            .text_size(px(11.0))
                            .text_color(rgb(if sp.matches.is_empty() && is_file {
                                DIM
                            } else {
                                TEXT
                            }))
                            .child(counter.clone()),
                    )
                    .child(chip("Aa", case, "search-toggle-case"))
                    .child(chip("ab", word, "search-toggle-word"))
                    .child(chip(".*", regex, "search-toggle-regex"))
                    .child(chip("⇕", show_replace, "search-toggle-replace"))
                    .child(
                        div()
                            .id("search-prev")
                            .debug_selector(|| "search-prev".to_string())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, e, _w, cx| {
                                    cx.stop_propagation();
                                    this.on_search_nav(false, e, _w, cx);
                                }),
                            )
                            .px_2()
                            .py_1()
                            .text_size(px(11.0))
                            .rounded_sm()
                            .bg(rgb(CLOSE_HOVER))
                            .text_color(rgb(TAB_INACTIVE))
                            .child("‹"),
                    )
                    .child(
                        div()
                            .id("search-next")
                            .debug_selector(|| "search-next".to_string())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, e, _w, cx| {
                                    cx.stop_propagation();
                                    this.on_search_nav(true, e, _w, cx);
                                }),
                            )
                            .px_2()
                            .py_1()
                            .text_size(px(11.0))
                            .rounded_sm()
                            .bg(rgb(CLOSE_HOVER))
                            .text_color(rgb(TAB_INACTIVE))
                            .child("›"),
                    )
                    .child(
                        div()
                            .id("search-close")
                            .debug_selector(|| "search-close".to_string())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, e, window, cx| {
                                    cx.stop_propagation();
                                    this.on_search_close(e, window, cx);
                                }),
                            )
                            .px_2()
                            .py_1()
                            .text_size(px(11.0))
                            .rounded_sm()
                            .hover(|d| d.bg(rgb(CLOSE_HOVER)))
                            .text_color(rgb(TAB_INACTIVE))
                            .child("×"),
                    ),
            );

        // Строка 2 (раскрыто): замена (File и Folder) + Replace/All
        // (только File — замена по вкладкам не предусматривается).
        if show_replace && (is_file || is_folder) {
            let mut row = div()
                .flex()
                .items_center()
                .gap_1()
                .pl(px(86.0))
                .child(
                    field_box(
                        replacement.clone(),
                        "Replace with…",
                        field == SearchField::Replace,
                        "search-field-replace",
                    )
                    .flex_1(),
                );
            if is_file {
                row = row
                    .child(
                        div()
                            .id("search-replace-one")
                            .debug_selector(|| "search-replace-one".to_string())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, e, _w, cx| {
                                    cx.stop_propagation();
                                    this.on_search_replace_button(false, e, _w, cx);
                                }),
                            )
                            .px_2()
                            .py_1()
                            .text_size(px(11.0))
                            .rounded_sm()
                            .bg(rgb(CLOSE_HOVER))
                            .text_color(rgb(TEXT))
                            .hover(|d| d.bg(rgb(ACCENT)))
                            .child("Replace"),
                    )
                    .child(
                        div()
                            .id("search-replace-all")
                            .debug_selector(|| "search-replace-all".to_string())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, e, _w, cx| {
                                    cx.stop_propagation();
                                    this.on_search_replace_button(true, e, _w, cx);
                                }),
                            )
                            .px_2()
                            .py_1()
                            .text_size(px(11.0))
                            .rounded_sm()
                            .bg(rgb(CLOSE_HOVER))
                            .text_color(rgb(TEXT))
                            .hover(|d| d.bg(rgb(ACCENT)))
                            .child("All"),
                    );
            }
            panel = panel.child(row);
        }

        // Строка 3 (folder): маска + папка + смена папки + замена.
        if is_folder {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl(px(86.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(DIM))
                            .child("mask:"),
                    )
                    .child(
                        field_box(
                            mask.clone(),
                            "*.rs  *.{ts,tsx}",
                            field == SearchField::Mask,
                            "search-field-mask",
                        )
                        .flex_1(),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(DIM))
                            .max_w(px(120.0))
                            .overflow_hidden()
                            .child(folder_name.clone()),
                    )
                    .child(
                        div()
                            .id("search-choose-folder")
                            .debug_selector(|| "search-choose-folder".to_string())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, e, window, cx| {
                                    cx.stop_propagation();
                                    this.on_search_choose_folder(e, window, cx);
                                }),
                            )
                            .px_2()
                            .py_1()
                            .text_size(px(11.0))
                            .rounded_sm()
                            .bg(rgb(CLOSE_HOVER))
                            .text_color(rgb(TAB_INACTIVE))
                            .hover(|d| d.bg(rgb(ACCENT)))
                            .child("…"),
                    )
                    .child(
                        div()
                            .id("search-folder-replace")
                            .debug_selector(|| "search-folder-replace".to_string())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(Self::on_search_replace_in_folder),
                            )
                            .px_2()
                            .py_1()
                            .text_size(px(11.0))
                            .rounded_sm()
                            .bg(rgb(CLOSE_HOVER))
                            .text_color(rgb(TEXT))
                            .hover(|d| d.bg(rgb(ACCENT)))
                            .child("Replace All…"),
                    ),
            );
        }

        // Ошибка regex / статус скана.
        if let Some(err) = error {
            panel = panel.child(
                div()
                    .id("search-error")
                    .debug_selector(|| "search-error".to_string())
                    .pl(px(86.0))
                    .text_size(px(11.0))
                    .text_color(rgb(SEARCH_ERROR))
                    .child(format!("Invalid pattern: {err}")),
            );
        } else if scanning {
            panel = panel.child(
                div()
                    .id("search-scanning")
                    .debug_selector(|| "search-scanning".to_string())
                    .pl(px(86.0))
                    .text_size(px(11.0))
                    .text_color(rgb(DIM))
                    .child("Scanning…"),
            );
        } else if !is_file {
            let text = match (&summary, results_len) {
                (Some(s), _) => format!(
                    "{} match(es) in {} file(s){}",
                    if results_len > 0 { results_len.to_string() } else { "0".into() },
                    s.files_with_hits,
                    if truncated { " (truncated)" } else { "" }
                ),
                (None, 0) => "Press Enter to search".to_string(),
                (None, n) => format!("{n} match(es)"),
            };
            panel = panel.child(
                div()
                    .id("search-summary")
                    .debug_selector(|| "search-summary".to_string())
                    .pl(px(86.0))
                    .text_size(px(11.0))
                    .text_color(rgb(DIM))
                    .child(text),
            );
        }
        panel.into_any_element()
    }

    /// Секция SEARCH левой панели (по образцу BOOKMARKS, D9.4):
    /// результаты Tabs/Folder поиска, клик — переход к совпадению.
    pub(crate) fn render_search_section(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let sp = self.search.as_ref()?;
        if sp.scope == SearchScope::File {
            return None;
        }
        let mut section = div()
            .mt_2()
            .border_t_1()
            .border_color(rgb(CLOSE_HOVER))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(11.0))
                    .text_color(rgb(PANEL_HEADER))
                    .child(if sp.scanning {
                        format!("SEARCH ({}… scanning)", sp.results.len())
                    } else {
                        format!("SEARCH ({})", sp.results.len())
                    }),
            );
        let mut last_owner: Option<String> = None;
        // Панель не virtualized: строим не больше SEARCH_SECTION_MAX_ROWS
        // записей (баг ручной проверки: тысячи div на каждый кадр),
        // остаток — строкой «…and N more».
        let shown = sp.results.len().min(SEARCH_SECTION_MAX_ROWS);
        let hidden = sp.results.len() - shown;
        for (hi, hit) in sp.results.iter().take(shown).enumerate() {
            let (label, is_tab) = match &hit.owner {
                SearchOwner::Tab { label, .. } => (label.clone(), true),
                SearchOwner::Path { label, .. } => (label.clone(), false),
            };
            // Заголовок группы при смене владельца.
            if last_owner.as_deref() != Some(label.as_str()) {
                last_owner = Some(label.clone());
                section = section.child(
                    div()
                        .px_3()
                        .pt_1()
                        .pb(px(1.0))
                        .text_size(px(10.0))
                        .text_color(rgb(if is_tab { ACCENT } else { PANEL_HEADER }))
                        .child(label.clone()),
                );
            }
            // Фрагмент: до / совпадение / после.
            let before: String = hit.fragment.chars().take(hit.frag_col).collect();
            let matched: String = hit
                .fragment
                .chars()
                .skip(hit.frag_col)
                .take(hit.frag_len)
                .collect();
            let after: String = hit
                .fragment
                .chars()
                .skip(hit.frag_col + hit.frag_len)
                .collect();
            section = section.child(
                div()
                    .id(("search-hit", hi))
                    .debug_selector(move || format!("search-hit:{hi}").to_string())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _e, _w, cx| {
                            cx.stop_propagation();
                            this.goto_search_hit(hi, cx);
                        }),
                    )
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_3()
                    .py(px(1.0))
                    .text_size(px(11.0))
                    .hover(|d| d.bg(rgb(CLOSE_HOVER)))
                    .child(
                        div()
                            .text_color(rgb(ACCENT))
                            .flex_shrink_0()
                            .child(format!("Ln {}", hit.line + 1)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .text_color(rgb(TAB_INACTIVE))
                            .child(
                                div()
                                    .flex()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(div().child(before))
                                    .child(div().bg(rgb(SEARCH_MATCH)).text_color(rgb(TEXT)).child(matched))
                                    .child(div().child(after)),
                            ),
                    ),
            );
        }
        if hidden > 0 {
            section = section.child(
                div()
                    .id("search-more")
                    .debug_selector(|| "search-more".to_string())
                    .px_3()
                    .py_1()
                    .text_size(px(10.0))
                    .text_color(rgb(PANEL_HINT))
                    .child(format!(
                        "…and {hidden} more{}",
                        if sp.truncated { " (results truncated)" } else { "" }
                    )),
            );
        }
        Some(section)
    }
}
