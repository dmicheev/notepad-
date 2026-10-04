//! notepad+ — точка входа (группа 3): восстановление сессии, окно
//! Workspace, глобальные keybindings и меню, сохранение сессии при
//! выходе (Cmd+Q / закрытие окна).

mod actions;
mod bench;
mod editor_actions;
mod editor_mouse;
mod editor_view;
mod folding;
mod geometry;
mod goto_dialog;
mod language;
mod language_dialog;
mod perf;
mod project_panel;
mod session;
mod status_bar;
mod tab_bar;
mod tab_model;
mod theme;
mod workspace;

#[cfg(test)]
mod gui_tests;
#[cfg(test)]
mod test_support;

use gpui::{
    App, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, point, prelude::*, px, size,
};

use crate::actions::{
    AddNextMatch, CloseTab, FoldAll, GoToLine, LowerCase, NextBookmark, OpenFile, PrevBookmark,
    Quit, Redo, RemoveDuplicateLines, RemoveEmptyLines, SaveFile, SaveFileAs, SentenceCase,
    SortAsc, SortAscIgnoreCase, SortDesc, SortDescIgnoreCase, TitleCase, ToggleBookmark, Undo,
    UnfoldAll, UpperCase,
};
use crate::session::{RestorePlan, SessionState};
use crate::workspace::Workspace;

fn main() {
    bench::enable_from_env();
    let app = Application::new();
    // macOS: клик по иконке в доке при закрытом окне открывает окно
    // заново с восстановлением сессии (приложение не завершается
    // закрытием последнего окна — D8.4).
    app.on_reopen(|cx: &mut App| {
        if cx.windows().is_empty() {
        open_workspace_window(cx, None);
        // Бенчмарк-стенд (D11.1): активировать приложение ПОСЛЕ создания
        // окна — на macOS перерисовка gpui идёт через CVDisplayLink,
        // который стартует только для видимого/активного окна; без
        // активации при запуске из CLI стенд не получит кадры после
        // первого (display link не запускается).
        if bench::enabled() {
            cx.activate(true);
        }
        }
    });
    app.run(|cx: &mut App| {
        // Минимальные горячие клавиши группы 3 (+undo/redo — часть
        // editing-пути через History; полный справочник — 8.5, D8.1).
        cx.bind_keys([
            KeyBinding::new("cmd-o", OpenFile, None),
            KeyBinding::new("cmd-s", SaveFile, None),
            KeyBinding::new("cmd-shift-s", SaveFileAs, None),
            KeyBinding::new("cmd-w", CloseTab, None),
            KeyBinding::new("cmd-z", Undo, None),
            KeyBinding::new("cmd-shift-z", Redo, None),
            KeyBinding::new("cmd-q", Quit, None),
            // Горячие клавиши группы 4 (только заявленные в 8.5):
            // Cmd+G — переход к строке (4.2); Cmd+D — следующий курсор
            // на совпадение слова (4.3).
            KeyBinding::new("cmd-g", GoToLine, None),
            KeyBinding::new("cmd-d", AddNextMatch, None),
            // Закладки (4.4): Cmd+F2 — toggle, F2/Shift+F2 — вперёд/назад.
            KeyBinding::new("cmd-f2", ToggleBookmark, None),
            KeyBinding::new("f2", NextBookmark, None),
            KeyBinding::new("shift-f2", PrevBookmark, None),
        ]);
        cx.set_menus(vec![Menu {
            name: "notepad+".into(),
            items: vec![
                MenuItem::action("Open…", OpenFile),
                MenuItem::action("Save", SaveFile),
                MenuItem::action("Save As…", SaveFileAs),
                MenuItem::action("Close Tab", CloseTab),
                MenuItem::action("Undo", Undo),
                MenuItem::action("Redo", Redo),
                MenuItem::action("Go to Line…", GoToLine),
                MenuItem::action("Toggle Bookmark", ToggleBookmark),
                MenuItem::action("Next Bookmark", NextBookmark),
                MenuItem::action("Previous Bookmark", PrevBookmark),
                MenuItem::action("Quit", Quit),
            ],
        },
        // Вид: свёртывание блоков (4.5). Горячих клавиш нет — полный
        // справочник в 8.5.
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Fold All", FoldAll),
                MenuItem::action("Unfold All", UnfoldAll),
            ],
        },
        // Текст: операции со строками (4.6). Горячих клавиш нет — 8.5.
        Menu {
            name: "Text".into(),
            items: vec![
                MenuItem::action("Sort Lines Ascending", SortAsc),
                MenuItem::action("Sort Lines Descending", SortDesc),
                MenuItem::action("Sort Lines Ascending (Ignore Case)", SortAscIgnoreCase),
                MenuItem::action("Sort Lines Descending (Ignore Case)", SortDescIgnoreCase),
                MenuItem::action("Remove Duplicate Lines", RemoveDuplicateLines),
                MenuItem::action("Remove Empty Lines", RemoveEmptyLines),
                MenuItem::separator(),
                MenuItem::action("UPPER CASE", UpperCase),
                MenuItem::action("lower case", LowerCase),
                MenuItem::action("Title Case", TitleCase),
                MenuItem::action("Sentence case", SentenceCase),
            ],
        },
    ]);

        open_workspace_window(cx, None);
    });
}

/// Открыть окно Workspace; `restore` — готовое состояние сессии (из
/// `on_reopen`), иначе читается с диска. Восстановление вкладок — по
/// плану `session::plan_restore` (несуществующие файлы пропускаются
/// с сообщением в статус-бар).
fn open_workspace_window(cx: &mut App, restore: Option<SessionState>) {
    let mut status: Option<String> = None;
    // Бенчмарк-стенд (D11.1): сессию не восстанавливаем — в замер
    // открытия NOTEPAD_OPEN не должны попадать чужие вкладки.
    let state = if bench::enabled() {
        None
    } else {
        match restore {
            Some(s) => Some(s),
            None => match session::session_path().map(|p| session::load_session(&p)) {
                Some(Ok(s)) => s,
                Some(Err(e)) => {
                    status = Some(format!("Could not read session: {e}"));
                    None
                }
                None => None,
            },
        }
    };

    // Геометрия окна: сохранённая (если осмысленная) или дефолт.
    let bounds = match state.as_ref().and_then(|s| s.window).filter(|g| g.is_sane()) {
        Some(g) => Bounds {
            origin: point(px(g.x), px(g.y)),
            size: size(px(g.width), px(g.height)),
        },
        None => Bounds::centered(None, size(px(1100.0), px(720.0)), cx),
    };

    let plan: Option<RestorePlan> =
        state.as_ref().map(|s| session::plan_restore(s, |p| p.exists()));

    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("notepad+".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_window, cx| {
                cx.new(|cx| {
                    let mut ws = Workspace::new(cx);
                    if let Some(plan) = &plan {
                        let mut open_error: Option<String> = None;
                        for path in &plan.tabs {
                            if let Err(e) = ws.model.open_path(path) {
                                eprintln!(
                                    "[session] could not open {}: {e}",
                                    path.display()
                                );
                                open_error.get_or_insert(format!(
                                    "Could not open {}: {e}",
                                    path.display()
                                ));
                            }
                        }
                        if let Some(a) = plan.active {
                            ws.model.activate(a);
                        }
                        if !plan.skipped.is_empty() {
                            let names: Vec<String> = plan
                                .skipped
                                .iter()
                                .map(|p| {
                                    p.file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| p.display().to_string())
                                })
                                .collect();
                            status = Some(format!(
                                "Skipped {} tab(s), file(s) no longer exist: {}",
                                plan.skipped.len(),
                                names.join(", ")
                            ));
                        }
                        if let Some(e) = open_error {
                            status = status.or(Some(e));
                        }
                        // Порог подсветки (5.3): уведомление, если
                        // восстановленная вкладка >20 МБ (без гонки с
                    }
                    // Восстановленные вкладки сверх порога — в очередь
                    // вопроса (диалог после создания окна, D12.4).
                    ws.queue_threshold_ask();
                    if let Some(m) = status.take() {
                        ws.status_message = Some(m);
                    }
                    // Бенчмарк 4.1: NOTEPAD_OPEN=<path> открывает файл при
                    // старте без диалога (ошибка — только в stderr). В режиме
                    // NOTEPAD_BENCH замеряется длительность BufferTab::open
                    // (load_file_detected + LineMeta) — D11.1.
                    if let Ok(path) = std::env::var("NOTEPAD_OPEN") {
                        let started = std::time::Instant::now();
                        let opened = ws.model.open_path(&path);
                        if bench::enabled() {
                            bench::record_load(started.elapsed());
                        }
                        if let Err(e) = opened {
                            eprintln!("[bench] could not open {path}: {e}");
                        } else {
                            // Порог подсветки (5.3/D12.4): вопрос
                            // пользователю (не в bench-режиме).
                            ws.queue_threshold_ask();
                        }
                    }
                    ws
                })
            },
        )
        .expect("open window");

    window
        .update(cx, |ws: &mut Workspace, window, cx| {
            window.focus(&ws.focus);
            // Вопрос о подсветке файлов сверх порога, накопленных при
            // восстановлении сессии/NOTEPAD_OPEN (D12.4).
            ws.ask_pending_threshold(window, cx);
            // Красная кнопка: сохранить сессию и разрешить закрытие;
            // приложение продолжает жить (dock-reopen откроет окно).
            let entity = cx.entity();
            window.on_window_should_close(cx, move |window, cx| {
                entity.read(cx).persist(window);
                true
            });
            // Бенчмарк-стенд (D11.1): async-драйвер скролла/ввода/undo
            // с ручным Window::draw (независимость от CVDisplayLink).
            // Запуск — после update ниже (см. bench::start).
        })
        .expect("update window");

    if bench::enabled() {
        bench::start(window.clone(), cx);
    }

    // Cmd+Q / платформенный выход: сохранить сессию (наблюдатель
    // on_app_quit покрывает все пути завершения).
    // leak: подписка должна жить всё время приложения.
    cx.on_app_quit({
        let window = window.clone();
        move |cx| {
            let _ = window.update(cx, |ws: &mut Workspace, window, _cx| {
                ws.persist(window);
            });
            std::future::ready(())
        }
    })
    .detach();
}
