//! notepad+ — точка входа (группа 3): восстановление сессии, окно
//! Workspace, глобальные keybindings и меню, сохранение сессии при
//! выходе (Cmd+Q / закрытие окна).

mod actions;
mod bench;
mod buffer_tab;
mod csv_dialog;
mod editor_actions;
mod editor_mouse;
mod editor_view;
mod folding;
mod font_dialog;
mod fs_tree;
mod fs_watch;
mod geometry;
mod goto_dialog;
mod i18n;
mod keymap;
mod language;
mod markdown_preview;
mod menus;
mod language_dialog;
mod overlay_input;
mod pane;
mod perf;
mod project_panel;
mod scrollbar;
mod search_panel;
mod search_render;
mod search_scan;
mod session;
mod status_bar;
mod tab_bar;
mod tab_model;
mod theme;
mod toolbar;
mod icons;
mod tree_panel;
mod workspace;
mod workspace_settings;

#[cfg(test)]
mod gui_tests;
#[cfg(test)]
mod test_support;

use gpui::{
    App, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, point,
    prelude::*, px, size,
};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use crate::actions::Quit;
use crate::session::{RestorePlan, SessionState};
use crate::workspace::Workspace;

/// Пути из odoc-событий («Открыть в приложении» ПОВЕРХ запущенного
/// приложения): gpui 0.2.2 отдаёт их в on_open_urls БЕЗ `&mut App` —
/// складываем в очередь, поллинг на главном потоке открывает.
static OPEN_REQUESTS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn main() {
    bench::enable_from_env();
    let app = Application::new().with_assets(crate::icons::EmbeddedAssets);
    // odoc-события («Открыть в приложении» ПОВЕРХ запущенного
    // приложения): gpui 0.2.2 отдаёт file:// URL без App-контекста —
    // складываем в очередь (открытие — поллингом внутри run).
    app.on_open_urls(|urls| {
        let mut queue = OPEN_REQUESTS.lock().unwrap();
        for url in urls {
            if let Some(path) = url_to_path(&url) {
                queue.push(path);
            }
        }
    });
    // macOS: клик по иконке в доке при закрытом окне открывает окно
    // заново с восстановлением сессии (приложение не завершается
    // закрытием последнего окна — D8.4).
    app.on_reopen(|cx: &mut App| {
        if cx.windows().is_empty() {
            open_workspace_window(cx, None, Vec::new());
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
        // i18n (8.4): язык интерфейса из сессии (en — дефолт);
        // внешние переводы — <data>/notepad-plus/locales/*.toml.
        let saved_lang = session::session_path()
            .and_then(|p| session::load_session(&p).ok().flatten())
            .and_then(|s| s.language);
        crate::i18n::init(cx, saved_lang.as_deref());
        // Минимальные горячие клавиши группы 3 (+undo/redo — часть
        // editing-пути через History; полный справочник — 8.5, D8.1).
        // Раскладка (8.5/D15.5): единый источник keymap::KEYMAP —
        // биндинги и справочник (Cmd+/) не расходятся. Платформенный
        // выход Cmd+Q — отдельно (стандартное место приложения).
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        crate::keymap::bind_all(cx);
        cx.set_menus(crate::menus::build(cx));

        // «Открыть в приложении» при старте: LaunchServices передаёт
        // пути аргументами командной строки. Файлы — вкладками,
        // каталог — воркспейсом дерева.
        let cli_paths: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
        open_workspace_window(cx, None, cli_paths);

        // Поллинг очереди odoc на главном потоке: пути открываются в
        // последнем окне (окон нет — создаётся новое).
        cx.spawn(async move |cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let paths: Vec<PathBuf> = std::mem::take(&mut *OPEN_REQUESTS.lock().unwrap());
            if paths.is_empty() {
                continue;
            }
            let _ = cx.update(|app| open_paths_in_active_window(app, paths));
        })
        .detach();
    });
}

/// file:// URL → путь: срезать схему/хост, percent-decode (пробелы,
/// кириллица в именах файлов). Не-file URL — None.
fn url_to_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            let byte = u8::from_str_radix(hex, 16).ok()?;
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
}

/// Открыть пути в ПОСЛЕДНЕМ окне («Открыть в приложении»); окон нет —
/// новое окно с этими путями.
fn open_paths_in_active_window(cx: &mut App, paths: Vec<PathBuf>) {
    let Some(any_window) = cx.windows().into_iter().next_back() else {
        open_workspace_window(cx, None, paths);
        return;
    };
    let Some(handle) = any_window.downcast::<Workspace>() else {
        return;
    };
    let _ = handle.update(cx, |ws, window, cx| {
        ws.open_paths(&paths, window, cx);
    });
}

/// Открыть окно Workspace; `restore` — готовое состояние сессии (из
/// `on_reopen`), иначе читается с диска. Восстановление вкладок — по
/// плану `session::plan_restore` (несуществующие файлы пропускаются
/// с сообщением в статус-бар). `startup` — файлы/каталог из аргументов
/// командной строки («Открыть с помощью» при холодном старте): файлы
/// открываются вкладками, первый каталог — воркспейсом дерева.
fn open_workspace_window(cx: &mut App, restore: Option<SessionState>, startup: Vec<PathBuf>) {
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
                    status = Some(crate::i18n::tf(cx, "status.session_read_failed", &[("err", &e.to_string())]));
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

    // Воркспейс (7.4): последний открытый восстанавливается при
    // старте; исчезнувшая папка — тихо пропускается (дерево можно
    // открыть заново через Cmd+Shift+O).
    let restore_workspace: Option<std::path::PathBuf> = state
        .as_ref()
        .and_then(|s| s.workspace.clone())
        .filter(|p| p.is_dir());

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
            |window, cx| {
                cx.new(|cx| {
                    let mut ws = Workspace::new(cx);
                    // Язык интерфейса (8.4) — зеркало глобали (для сессии).
                    ws.ui_language = crate::i18n::language(cx);
                    // Тема из сессии (8.2); System/неизвестная —
                    // системная настройка.
                    if let Some(kind) = state.as_ref().and_then(|s| s.theme.as_deref()) {
                        ws.theme_kind = crate::theme::ThemeKind::parse(kind);
                    }
                    // Тулбар из сессии (toolbar-and-shortcuts);
                    // старые сессии без поля — виден (default true).
                    if let Some(visible) = state.as_ref().and_then(|s| s.toolbar_visible) {
                        ws.toolbar_visible = visible;
                    }
                    // Шрифт из сессии (8.3).
                    let font_ok = |f: &session::FontSettings| {
                        f.size.is_finite()
                            && f.size >= crate::geometry::FONT_MIN
                            && f.size <= crate::geometry::FONT_MAX
                            && !f.family.trim().is_empty()
                    };
                    if let Some(font) = state
                        .as_ref()
                        .and_then(|s| s.font.clone())
                        .filter(|f| font_ok(f))
                    {
                        ws.font =
                            crate::geometry::FontConfig { family: font.family, size: font.size };
                    }
                    if let Some(plan) = &plan {
                        let mut open_error: Option<String> = None;
                        for path in &plan.tabs {
                            if let Err(e) = ws.model_mut().open_path(path) {
                                eprintln!(
                                    "[session] could not open {}: {e}",
                                    path.display()
                                );
                                open_error.get_or_insert(crate::i18n::tf(
                                    cx,
                                    "status.open_failed",
                                    &[
                                        ("file", &path.display().to_string()),
                                        ("err", &e.to_string()),
                                    ],
                                ));
                            }
                        }
                        if let Some(a) = plan.active {
                            ws.model_mut().activate(a);
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
                            status = Some(crate::i18n::tf(
                                cx,
                                "status.skipped_tabs",
                                &[
                                    ("n", &plan.skipped.len().to_string()),
                                    ("files", &names.join(", ")),
                                ],
                            ));
                        }
                        if let Some(e) = open_error {
                            status = status.or(Some(e));
                        }
                    }
                    // Восстановленные вкладки сверх порога — в очередь
                    // вопроса (диалог после создания окна, D12.4).
                    ws.queue_threshold_ask();
                    // CSV без сохранённого выбора — в очередь вопроса о
                    // разделителе/кавычках (D12.6; после открытия
                    // воркспейса ниже — сохранённый выбор не спрашивается).
                    ws.queue_csv_ask();
                    // Последний воркспейс (7.4): дерево + наблюдатель
                    // (bench-режим сессию не восстанавливает — путь
                    // остаётся None).
                    if let Some(root) = restore_workspace {
                        ws.open_workspace_folder(&root, cx);
                    }
                    if let Some(m) = status.take() {
                        ws.set_status_message(m, cx);
                    }
                    // Бенчмарк 4.1: NOTEPAD_OPEN=<path> открывает файл при
                    // старте без диалога (ошибка — только в stderr). В режиме
                    // NOTEPAD_BENCH замеряется длительность BufferTab::open
                    // (load_file_detected + LineMeta) — D11.1.
                    if let Ok(path) = std::env::var("NOTEPAD_OPEN") {
                        let started = std::time::Instant::now();
                        let opened = ws.model_mut().open_path(&path);
                        if bench::enabled() {
                            bench::record_load(started.elapsed());
                        }
                        if let Err(e) = opened {
                            eprintln!("[bench] could not open {path}: {e}");
                        } else {
                            // Порог подсветки (5.3/D12.4): вопрос
                            // пользователю (не в bench-режиме).
                            ws.queue_threshold_ask();
                            // CSV (D12.6): вопрос о разделителе/кавычках.
                            ws.queue_csv_ask();
                        }
                    }
                    // Пути из командной строки («Открыть с помощью»
                    // при холодном старте, запуск `notepad-plus file`):
                    // файлы — вкладками (open_paths сам активирует
                    // панель и ставит в очередь вопрос о пороге),
                    // первый каталог — воркспейсом дерева.
                    if !startup.is_empty() {
                        let files: Vec<PathBuf> =
                            startup.iter().filter(|p| p.is_file()).cloned().collect();
                        if !files.is_empty() {
                            ws.open_paths(&files, window, cx);
                        }
                        if let Some(dir) = startup.iter().find(|p| p.is_dir()) {
                            ws.open_workspace_folder(dir, cx);
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
            // Вопрос о настройках CSV из той же очереди (D12.6).
            ws.ask_pending_csv(window, cx);
            // Красная кнопка: сохранить сессию и разрешить закрытие;
            // приложение продолжает жить (dock-reopen откроет окно).
            let entity = cx.entity();
            window.on_window_should_close(cx, move |window, cx| {
                let _ = entity.update(cx, |ws, _| ws.persist(window));
                true
            });
            // Бенчмарк-стенд (D11.1): async-драйвер скролла/ввода/undo
            // с ручным Window::draw (независимость от CVDisplayLink).
            // Запуск — после update ниже (см. bench::start).
        })
        .expect("update window");

    if bench::enabled() {
        bench::start(window, cx);
    }

    // Cmd+Q / платформенный выход: сохранить сессию (наблюдатель
    // on_app_quit покрывает все пути завершения).
    // leak: подписка должна жить всё время приложения.
    cx.on_app_quit(move |cx| {
        let _ = window.update(cx, |ws: &mut Workspace, window, _cx| {
            ws.persist(window);
        });
        std::future::ready(())
    })
    .detach();
}

#[cfg(test)]
mod open_with_tests {
    use super::*;

    /// file:// URL → путь (odoc «Открыть в приложении»).
    #[test]
    fn file_url_plain_path() {
        assert_eq!(
            url_to_path("file:///Users/x/project/main.rs"),
            Some(PathBuf::from("/Users/x/project/main.rs"))
        );
    }

    /// percent-encoding: пробелы и кириллица в имени файла.
    #[test]
    fn file_url_percent_decoding() {
        assert_eq!(
            url_to_path("file:///Users/x/My%20Notes/%D1%84%D0%B0%D0%B9%D0%BB.txt"),
            Some(PathBuf::from("/Users/x/My Notes/файл.txt"))
        );
    }

    /// localhost-хост допускается; не-file URL отвергается.
    #[test]
    fn localhost_host_and_non_file() {
        assert_eq!(
            url_to_path("file://localhost/tmp/a.json"),
            Some(PathBuf::from("/tmp/a.json"))
        );
        assert_eq!(url_to_path("https://example.com/a.rs"), None);
        assert_eq!(url_to_path("plain/path"), None);
    }

    /// Разбор стартовых путей: файлы → вкладки, каталог → воркспейс.
    #[test]
    fn startup_paths_split_files_and_dirs() {
        let dir = std::env::temp_dir().join("notepad-startup-split");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.rs"), "fn main() {}").unwrap();
        let startup = vec![dir.join("a.rs"), dir.join("sub")];
        let files: Vec<PathBuf> = startup.iter().filter(|p| p.is_file()).cloned().collect();
        let found_dir = startup.iter().find(|p| p.is_dir()).cloned();
        assert_eq!(files, vec![dir.join("a.rs")]);
        assert_eq!(found_dir, Some(dir.join("sub")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
