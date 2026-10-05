//! Рендер панели поиска (6.1, D13): абсолютный оверлей top-right
//! редактора (чипы/поля/счётчик/навигация) и секция SEARCH левой
//! панели (результаты Tabs/Folder). Логика и состояние —
//! search_panel.rs; фоновые сканы — search_scan.rs.

use gpui::{Context, MouseButton, div, prelude::*, px, rgb};

use crate::search_panel::*;
use crate::search_panel::SEARCH_PANEL_W;
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
        let counter = sp.counter(&crate::i18n::t(cx, "search.no_results"));
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

        let field_box = |text: String, placeholder: String, active: bool, id: &'static str| {
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
                .bg(rgb(crate::theme::current().bg))
                .border_1()
                .border_color(rgb(if active { crate::theme::current().accent } else { crate::theme::current().close_hover }))
                .text_size(px(12.0))
                .text_color(rgb(if text.is_empty() { crate::theme::current().dim } else { crate::theme::current().text }))
                .child(if text.is_empty() { placeholder } else { text })
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
                .when(active, |d| d.bg(rgb(crate::theme::current().accent)).text_color(rgb(crate::theme::current().status_fg)))
                .when(!active, |d| d.bg(rgb(crate::theme::current().close_hover)).text_color(rgb(crate::theme::current().tab_inactive)))
                .hover(|d| d.bg(rgb(if active { crate::theme::current().status_lang_hover } else { crate::theme::current().close_hover })))
                .child(label)
        };

        // Строка 1: scope-чипы + запрос + счётчик + опции + навигация.
        let scope_chip = |label: String, active: bool, id: &'static str| {
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
                .when(active, |d| d.bg(rgb(crate::theme::current().accent)).text_color(rgb(crate::theme::current().status_fg)))
                .when(!active, |d| d.bg(rgb(crate::theme::current().close_hover)).text_color(rgb(crate::theme::current().tab_inactive)))
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
            .bg(rgb(crate::theme::current().tabbar))
            .border_1()
            .border_color(rgb(crate::theme::current().close_hover))
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
                    .child(scope_chip(crate::i18n::t(cx, "search.scope_file"), is_file, "search-scope-file"))
                    .child(scope_chip(crate::i18n::t(cx, "search.scope_tabs"), sp.scope == SearchScope::Tabs, "search-scope-tabs"))
                    .child(scope_chip(
                        crate::i18n::t(cx, "search.scope_folder"),
                        is_folder,
                        "search-scope-folder",
                    ))
                    .child(
                        field_box(
                            query.clone(),
                            if is_file { crate::i18n::t(cx, "search.placeholder") } else { crate::i18n::t(cx, "search.placeholder_enter") },
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
                            .bg(rgb(crate::theme::current().close_hover))
                            .text_color(rgb(crate::theme::current().text))
                            .hover(|d| d.bg(rgb(crate::theme::current().accent)).text_color(rgb(crate::theme::current().status_fg)))
                            .child("🔍"),
                    )
                    .child(
                        div()
                            .id("search-counter")
                            .debug_selector(|| "search-counter".to_string())
                            .px_1()
                            .text_size(px(11.0))
                            .text_color(rgb(if sp.matches.is_empty() && is_file {
                                crate::theme::current().dim
                            } else {
                                crate::theme::current().text
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
                            .bg(rgb(crate::theme::current().close_hover))
                            .text_color(rgb(crate::theme::current().tab_inactive))
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
                            .bg(rgb(crate::theme::current().close_hover))
                            .text_color(rgb(crate::theme::current().tab_inactive))
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
                            .hover(|d| d.bg(rgb(crate::theme::current().close_hover)))
                            .text_color(rgb(crate::theme::current().tab_inactive))
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
                        crate::i18n::t(cx, "search.replace_placeholder"),
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
                            .bg(rgb(crate::theme::current().close_hover))
                            .text_color(rgb(crate::theme::current().text))
                            .hover(|d| d.bg(rgb(crate::theme::current().accent)))
                            .child(crate::i18n::t(cx, "search.replace")),
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
                            .bg(rgb(crate::theme::current().close_hover))
                            .text_color(rgb(crate::theme::current().text))
                            .hover(|d| d.bg(rgb(crate::theme::current().accent)))
                            .child(crate::i18n::t(cx, "search.replace_all_button")),
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
                            .text_color(rgb(crate::theme::current().dim))
                            .child(crate::i18n::t(cx, "search.mask_label")),
                    )
                    .child(
                        field_box(
                            mask.clone(),
                            crate::i18n::t(cx, "search.mask_placeholder"),
                            field == SearchField::Mask,
                            "search-field-mask",
                        )
                        .flex_1(),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(crate::theme::current().dim))
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
                            .bg(rgb(crate::theme::current().close_hover))
                            .text_color(rgb(crate::theme::current().tab_inactive))
                            .hover(|d| d.bg(rgb(crate::theme::current().accent)))
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
                            .bg(rgb(crate::theme::current().close_hover))
                            .text_color(rgb(crate::theme::current().text))
                            .hover(|d| d.bg(rgb(crate::theme::current().accent)))
                            .child(crate::i18n::t(cx, "search.replace_in_files")),
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
                    .text_color(rgb(crate::theme::current().search_error))
                    .child(crate::i18n::tf(cx, "search.invalid_pattern", &[("err", &err.to_string())])),
            );
        } else if scanning {
            panel = panel.child(
                div()
                    .id("search-scanning")
                    .debug_selector(|| "search-scanning".to_string())
                    .pl(px(86.0))
                    .text_size(px(11.0))
                    .text_color(rgb(crate::theme::current().dim))
                    .child(crate::i18n::t(cx, "search.scanning")),
            );
        } else if !is_file {
            let text = match (&summary, results_len) {
                (Some(s), _) => format!(
                    "{}{}",
                    crate::i18n::tf(
                        cx,
                        "search.summary",
                        &[
                            ("hits", &results_len.to_string()),
                            ("files", &s.files_with_hits.to_string()),
                        ],
                    ),
                    if truncated { crate::i18n::t(cx, "search.summary_truncated_suffix") } else { String::new() },
                ),
                (None, 0) => crate::i18n::t(cx, "search.press_enter"),
                (None, n) => crate::i18n::tf(cx, "search.summary_hits_only", &[("n", &n.to_string())]),
            };
            panel = panel.child(
                div()
                    .id("search-summary")
                    .debug_selector(|| "search-summary".to_string())
                    .pl(px(86.0))
                    .text_size(px(11.0))
                    .text_color(rgb(crate::theme::current().dim))
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
            .border_color(rgb(crate::theme::current().close_hover))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(11.0))
                    .text_color(rgb(crate::theme::current().panel_header))
                    .child(if sp.scanning {
                        crate::i18n::tf(cx, "panel.search_scanning", &[("n", &sp.results.len().to_string())])
                    } else {
                        crate::i18n::tf(cx, "panel.search", &[("n", &sp.results.len().to_string())])
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
                        .text_color(rgb(if is_tab { crate::theme::current().accent } else { crate::theme::current().panel_header }))
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
                    .hover(|d| d.bg(rgb(crate::theme::current().close_hover)))
                    .child(
                        div()
                            .text_color(rgb(crate::theme::current().accent))
                            .flex_shrink_0()
                            .child(format!("Ln {}", hit.line + 1)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .text_color(rgb(crate::theme::current().tab_inactive))
                            .child(
                                div()
                                    .flex()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(div().child(before))
                                    .child(div().bg(rgb(crate::theme::current().search_match)).text_color(rgb(crate::theme::current().text)).child(matched))
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
                    .text_color(rgb(crate::theme::current().panel_hint))
                    .child(format!(
                        "…and {hidden} more{}",
                        if sp.truncated { " (results truncated)" } else { "" }
                    )),
            );
        }
        Some(section)
    }
}
