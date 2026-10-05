//! Меню приложения (8.4/D15.4): построение из i18n-ключей — смена
//! языка пересобирает меню (`i18n::set_language`); один источник для
//! старта (main) и переключения языка.

use gpui::{Menu, MenuItem};

use crate::actions::*;
use crate::i18n;

/// Собрать меню с текущим языком (main при старте; смена языка).
pub(crate) fn build(cx: &gpui::App) -> Vec<Menu> {
    let t = |k: &str| i18n::t(cx, k);
    vec![
        // App-меню (macOS): только выход; всё остальное — по
        // стандартным меню ниже (правка ручной проверки: единый
        // список в app-меню не давал привычного «File»).
        Menu {
            name: "notepad+".into(),
            items: vec![MenuItem::action(t("menu.app.quit"), Quit)],
        },
        // Файлы и воркспейс: открытие/сохранение/вкладки (3.x, 7.x)
        // + создание в дереве (7.2).
        Menu {
            name: t("menu.name.file").into(),
            items: vec![
                MenuItem::action(t("menu.file.new_file"), NewFile),
                MenuItem::action(t("menu.file.new_folder"), NewFolder),
                MenuItem::separator(),
                MenuItem::action(t("menu.file.open"), OpenFile),
                MenuItem::action(t("menu.file.open_workspace"), OpenWorkspace),
                MenuItem::separator(),
                MenuItem::action(t("menu.file.save"), SaveFile),
                MenuItem::action(t("menu.file.save_as"), SaveFileAs),
                MenuItem::separator(),
                MenuItem::action(t("menu.file.close_tab"), CloseTab),
            ],
        },
        // Правка: undo/redo (2.4) — стандартное место.
        Menu {
            name: t("menu.name.edit").into(),
            items: vec![
                MenuItem::action(t("menu.edit.undo"), Undo),
                MenuItem::action(t("menu.edit.redo"), Redo),
            ],
        },
        // Навигация: переход к строке (4.2) и закладки (4.4).
        Menu {
            name: t("menu.name.navigate").into(),
            items: vec![
                MenuItem::action(t("menu.navigate.go_to_line"), GoToLine),
                MenuItem::separator(),
                MenuItem::action(t("menu.navigate.toggle_bookmark"), ToggleBookmark),
                MenuItem::action(t("menu.navigate.next_bookmark"), NextBookmark),
                MenuItem::action(t("menu.navigate.prev_bookmark"), PrevBookmark),
            ],
        },
        // Вид: split view (8.1), темы/шрифт/язык (8.2–8.4),
        // свёртывание блоков (4.5).
        Menu {
            name: t("menu.name.view").into(),
            items: vec![
                MenuItem::action(t("menu.view.split_editor"), SplitEditor),
                MenuItem::action(t("menu.view.close_pane"), ClosePane),
                MenuItem::action(t("menu.view.sync_scroll"), ToggleSyncScroll),
                MenuItem::separator(),
                MenuItem::submenu(Menu {
                    name: t("menu.name.theme").into(),
                    items: vec![
                        MenuItem::action(t("menu.view.theme.system"), ThemeSystem),
                        MenuItem::action(t("menu.view.theme.dark"), ThemeDark),
                        MenuItem::action(t("menu.view.theme.light"), ThemeLight),
                    ],
                }),
                MenuItem::action(t("menu.view.font"), OpenFontDialog),
                MenuItem::submenu(Menu {
                    name: t("menu.view.interface_language").into(),
                    items: vec![MenuItem::action(
                        t("menu.view.choose_language"),
                        OpenUiLanguageDialog,
                    )],
                }),
                MenuItem::separator(),
                MenuItem::action(t("menu.view.fold_all"), FoldAll),
                MenuItem::action(t("menu.view.unfold_all"), UnfoldAll),
            ],
        },
        // Поиск (6): в файле / по вкладкам / по папке; замена — в панели.
        Menu {
            name: t("menu.name.search").into(),
            items: vec![
                MenuItem::action(t("menu.search.find"), FindInFile),
                MenuItem::action(t("menu.search.find_in_tabs"), FindInTabs),
                MenuItem::action(t("menu.search.find_in_folder"), FindInFolder),
            ],
        },
        // Текст: операции со строками (4.6).
        Menu {
            name: t("menu.name.text").into(),
            items: vec![
                MenuItem::action(t("menu.text.sort_asc"), SortAsc),
                MenuItem::action(t("menu.text.sort_desc"), SortDesc),
                MenuItem::action(t("menu.text.sort_asc_ic"), SortAscIgnoreCase),
                MenuItem::action(t("menu.text.sort_desc_ic"), SortDescIgnoreCase),
                MenuItem::action(t("menu.text.dedupe"), RemoveDuplicateLines),
                MenuItem::action(t("menu.text.remove_empty"), RemoveEmptyLines),
                MenuItem::separator(),
                MenuItem::action(t("menu.text.upper"), UpperCase),
                MenuItem::action(t("menu.text.lower"), LowerCase),
                MenuItem::action(t("menu.text.title"), TitleCase),
                MenuItem::action(t("menu.text.sentence"), SentenceCase),
            ],
        },
        // Справка: справочник сочетаний (8.5, Cmd+/).
        Menu {
            name: t("menu.help.name").into(),
            items: vec![MenuItem::action(t("menu.help.shortcuts"), ShowShortcuts)],
        },
    ]
}
