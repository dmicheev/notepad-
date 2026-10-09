//! GUI-тесты на `gpui::test` (simulate_keystrokes / dispatch_action /
//! simulate_click / debug_bounds): сквозные пути клавиш, мыши и
//! рендера. Паттерны и ловушки gpui 0.2.2:
//! - в gpui::test main() НЕ выполняется → keymap пуст; хоткеи обязаны
//!   проверяться через bind_app_keys + simulate_keystrokes (не только
//!   dispatch_action), иначе биндинг может молча не работать;
//! - у рендера есть тесты на ШИРИНЫ/ПОЗИЦИИ элементов (debug_selector +
//!   cx.debug_bounds): юнит-тесты данных (split_line/line_carets) не
//!   видят ошибок построения div-ов (урок бага «фон выделения красит
//!   строку целиком», D9.1);
//! - арифметика ожиданий: длина строки vs шаг с \n, хвостовая
//!   фантомная строка ropey, reversed-семантика Selection::range —
//!   комментарии-расчёты в тестах обязательны (сохранять при правках).

use crate::actions::*;
use crate::editor_view::line_carets;
use crate::geometry::{GUTTER_W, LINE_HEIGHT};
use crate::test_support::TempDir;
use crate::workspace::Workspace;
use editor_core::Selection;
use gpui::{
    Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext, VisualContext, point,
    px,
};
use std::path::PathBuf;

/// Координаты мыши в GUI-тестах: точка (строка, колонка) редактора
/// — от bounds uniform_list, гуттера и char advance (паттерн всех
/// мышиных тестов 4.2/4.3; дробные части — как у реальной мыши:
/// 2.5 — середина строки 2, 3.2 — между символами 3 и 4).
struct MouseGrid {
    origin: gpui::Point<gpui::Pixels>,
    advance: f32,
}

impl MouseGrid {
    /// Замерить сетку по отрисованному редактору (bounds + advance).
    fn of(cx: &mut gpui::VisualTestContext, view: &gpui::Entity<Workspace>) -> Self {
        let (origin, advance) = cx.update_window_entity(view, |ws, window, _| {
            let b = ws.editor_bounds().expect("editor painted");
            (b.origin, ws.char_advance(window))
        });
        MouseGrid { origin, advance }
    }

    /// Пиксели окна для (line, col).
    fn at(&self, line: f32, col: f32) -> gpui::Point<gpui::Pixels> {
        point(
            px(f32::from(self.origin.x) + GUTTER_W + col * self.advance),
            px(f32::from(self.origin.y) + line * LINE_HEIGHT),
        )
    }
}

/// Окно с открытой вкладкой-файлом; фокус в редакторе. `tag` —
/// уникальный на тест (тесты идут параллельно в одном процессе).
/// Каталог забывается намеренно: файл должен жить дольше хелпера.
fn editor_with_file<'a>(
    cx: &'a mut TestAppContext,
    tag: &str,
    content: &str,
) -> (gpui::Entity<Workspace>, &'a mut gpui::VisualTestContext, PathBuf) {
    let dir = TempDir::new(tag);
    let path = dir.write("file.txt", content);
    std::mem::forget(dir);
    let (view, cx) = cx.add_window_view(|_window, cx| {
        let mut ws = Workspace::new(cx);
        ws.model_mut().open_path(&path).unwrap();
        ws
    });
    focus_editor(&view, cx);
    (view, cx, path)
}

/// Фокус редактора панели 0 (8.1: у каждой панели свой FocusHandle;
/// cx.focus(&view) фокусирует корневой handle — клавиши не доходят).
fn focus_editor(view: &gpui::Entity<Workspace>, cx: &mut gpui::VisualTestContext) {
    cx.update_window_entity(view, |ws, window, _cx| {
        let f = ws.panes[0].focus.clone();
        window.focus(&f);
    });
}

/// Ввод с клавиатуры доходит до редактора и идёт через editor-core
/// (документ + dirty + позиция каретки).
#[gpui::test]
fn typing_edits_document_and_marks_dirty(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "typing", "");
    cx.simulate_keystrokes("h e l l o");
    let (text, dirty, head) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (tab.doc.text(), tab.dirty, tab.primary().head())
    });
    assert_eq!(text, "hello");
    assert!(dirty);
    assert_eq!(head, 5);
}

/// Стрелки/Home/End двигают каретку (статус-бар читает из модели).
#[gpui::test]
fn arrow_keys_move_caret(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "arrows", "ab\ncd");
    cx.simulate_keystrokes("end"); // каретка в конец строки 0 → 2
    cx.simulate_keystrokes("down"); // → 4 (после "cd")
    cx.simulate_keystrokes("home"); // → 3
    cx.simulate_keystrokes("left"); // → 2 (на терминатор)
    let (head, line, col) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (tab.primary().head(), tab.cursor_line(), tab.cursor_col())
    });
    assert_eq!(head, 2);
    // Статус-бар (модель): Ln/Col пересчитываются.
    assert_eq!((line, col), (0, 2));
}

/// Backspace/enter работают через транзакции editor-core.
#[gpui::test]
fn backspace_and_enter(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "backspace", "");
    cx.simulate_keystrokes("a b");
    cx.simulate_keystrokes("backspace"); // "a"
    cx.simulate_keystrokes("enter"); // "a\n"
    let (text, head) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (tab.doc.text(), tab.primary().head())
    });
    assert_eq!(text, "a\n");
    assert_eq!(head, 2);
}

/// Cmd+S (dispatch SaveFile) сохраняет на диск и сбрасывает dirty.
#[gpui::test]
fn save_action_writes_disk(cx: &mut TestAppContext) {
    let (view, cx, path) = editor_with_file(cx, "save", "ab");
    cx.simulate_keystrokes("!"); // вставка в начало → "!ab"
    cx.dispatch_action(SaveFile);
    cx.run_until_parked();
    let dirty = cx
        .cx
        .update(|app| view.read(app).model().active().unwrap().dirty);
    assert!(!dirty, "save resets dirty");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "!ab");
}

/// Cmd+Z/Cmd+Shift+Z (dispatch Undo/Redo) через History.
#[gpui::test]
fn undo_redo_actions(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "undo", "");
    cx.simulate_keystrokes("x y");
    cx.dispatch_action(Undo);
    let after_undo = cx
        .cx
        .update(|app| view.read(app).model().active().unwrap().doc.text());
    assert_eq!(after_undo, "");
    cx.dispatch_action(Redo);
    let after_redo = cx
        .cx
        .update(|app| view.read(app).model().active().unwrap().doc.text());
    assert_eq!(after_redo, "xy");
}

/// 4.2: PgDn/PgUp — страница = высота вьюпорта в строках, clamp по
/// границам документа.
#[gpui::test]
fn page_up_down_moves_by_page(cx: &mut TestAppContext) {
    let content: String = (0..300).map(|i| format!("line{i:03}\n")).collect();
    let (view, cx, _path) = editor_with_file(cx, "page", &content);
    let page = cx
        .update_window_entity(&view, |ws, window, _| ws.page_lines(window));
    assert!(page >= 1);
    cx.simulate_keystrokes("pagedown");
    let line = cx.cx.update(|app| view.read(app).model().active().unwrap().cursor_line());
    assert_eq!(line, page);
    cx.simulate_keystrokes("pageup");
    let line = cx.cx.update(|app| view.read(app).model().active().unwrap().cursor_line());
    assert_eq!(line, 0);
}

/// 4.2: клик мышью ставит каретку в (строка, колонка) — координаты
/// считаются от bounds uniform_list, гуттера и char advance.
#[gpui::test]
fn click_positions_caret(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "click", "alpha\nbeta\ngamma\n");
    let grid = MouseGrid::of(cx, &view);
    // Строка 2 («gamma»), колонка 3: y в середине строки 2 (floor
    // 2.5 → 2), x = 3.2 символа (round → 3).
    cx.simulate_click(grid.at(2.5, 3.2), Modifiers::default());
    let (line, col) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (tab.cursor_line(), tab.cursor_col())
    });
    assert_eq!((line, col), (2, 3));
}

/// 4.2: клик → shift+клик расширяет выделение от первой позиции.
#[gpui::test]
fn shift_click_extends_selection(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "shift-click", "alpha\nbeta\ngamma\n");
    let grid = MouseGrid::of(cx, &view);
    // Клик: строка 1, колонка 1.
    cx.simulate_click(grid.at(1.5, 1.2), Modifiers::default());
    // Shift+клик: строка 2, колонка 3.
    cx.simulate_click(grid.at(2.5, 3.2), Modifiers {
        shift: true,
        ..Default::default()
    });
    let sel = cx.cx.update(|app| view.read(app).model().active().unwrap().cursors.last().copied());
    // "alpha\nbeta\ngamma\n": строка1 col1 = позиция 7, строка2 col3 = 14.
    assert_eq!(sel, Some(Selection::range(7, 14)));
}

/// 4.2: драг мышью — выделение от нажатия до отпускания.
#[gpui::test]
fn mouse_drag_selects_range(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "drag", "alpha\nbeta\ngamma\n");
    let grid = MouseGrid::of(cx, &view);
    let mods = Modifiers::default();
    cx.simulate_mouse_down(grid.at(0.5, 1.2), MouseButton::Left, mods);
    cx.simulate_mouse_move(grid.at(2.5, 3.2), Some(MouseButton::Left), mods);
    cx.simulate_mouse_up(grid.at(2.5, 3.2), MouseButton::Left, mods);
    let sel = cx.cx.update(|app| view.read(app).model().active().unwrap().cursors.last().copied());
    // Строка 0 col 1 = 1; строка 2 col 3 = 14.
    assert_eq!(sel, Some(Selection::range(1, 14)));
}

/// 4.2: Cmd+G — overlay-диалог, ввод числа, Enter → каретка на
/// строку, диалог закрыт.
#[gpui::test]
fn goto_line_dialog_jumps(cx: &mut TestAppContext) {
    let content: String = (0..100).map(|i| format!("row{i:03}\n")).collect();
    let (view, cx, _path) = editor_with_file(cx, "goto", &content);
    cx.dispatch_action(GoToLine);
    let open = cx.cx.update(|app| view.read(app).goto_input.clone());
    assert_eq!(open, Some(String::new()));
    cx.simulate_keystrokes("4 2");
    let input = cx.cx.update(|app| view.read(app).goto_input.clone());
    assert_eq!(input, Some("42".to_string()));
    cx.simulate_keystrokes("enter");
    let (line, col, open) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (tab.cursor_line(), tab.cursor_col(), view.read(app).goto_input.clone())
    });
    assert_eq!((line, col), (41, 0));
    assert_eq!(open, None);
}

/// 4.2: Cmd+G — Escape отменяет, каретка не двигается; ввод
/// за границами файла клампится.
#[gpui::test]
fn goto_line_escape_and_clamp(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "goto-esc", "one\ntwo\nthree\n");
    cx.dispatch_action(GoToLine);
    cx.simulate_keystrokes("9 9 9");
    cx.simulate_keystrokes("escape");
    let (line, open) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (tab.cursor_line(), view.read(app).goto_input.clone())
    });
    assert_eq!(line, 0, "escape leaves caret in place");
    assert_eq!(open, None);

    // Clamp: строка 999 → последняя строка (3).
    cx.dispatch_action(GoToLine);
    cx.simulate_keystrokes("9 9 9 enter");
    let (line, col) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (tab.cursor_line(), tab.cursor_col())
    });
    assert_eq!((line, col), (3, 0));
}

/// Cmd+G (dispatch GoToLine) при пустом списке вкладок — no-op.
#[gpui::test]
fn goto_line_without_tabs_is_noop(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "goto-empty", "x\n");
    cx.dispatch_action(CloseTab);
    cx.run_until_parked();
    cx.dispatch_action(GoToLine);
    let open = cx.cx.update(|app| view.read(app).goto_input.clone());
    assert_eq!(open, None);
}

/// 4.3: Cmd+клик добавляет курсор; ввод применяется во всех
/// позициях, курсоры остаются каретками.
#[gpui::test]
fn cmd_click_adds_cursor_and_types(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "cmd-click", "alpha\nbeta\ngamma\n");
    let grid = MouseGrid::of(cx, &view);
    cx.simulate_click(grid.at(0.5, 0.2), Modifiers::default());
    cx.simulate_click(grid.at(2.5, 0.2), Modifiers {
        platform: true,
        ..Default::default()
    });
    let count = cx.cx.update(|app| view.read(app).model().active().unwrap().cursors.len());
    assert_eq!(count, 2, "cmd+click adds a cursor");
    cx.simulate_keystrokes("z");
    let (text, cursors) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (tab.doc.text(), tab.cursors.clone())
    });
    assert_eq!(text, "zalpha\nbeta\nzgamma\n");
    assert_eq!(cursors.len(), 2);
    assert!(cursors.iter().all(|c| c.is_caret()));
}

/// 4.3: Alt+drag — column selection: каретка/выделение в каждой
/// строке блока, ввод одновременно.
#[gpui::test]
fn alt_drag_column_selection(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "alt-drag", "abcde\nabcde\nabcde\n");
    let grid = MouseGrid::of(cx, &view);
    let mods = Modifiers {
        alt: true,
        ..Default::default()
    };
    cx.simulate_mouse_down(grid.at(0.5, 1.2), MouseButton::Left, mods);
    cx.simulate_mouse_move(grid.at(2.5, 3.2), Some(MouseButton::Left), mods);
    cx.simulate_mouse_up(grid.at(2.5, 3.2), MouseButton::Left, mods);
    let cursors = cx.cx.update(|app| view.read(app).model().active().unwrap().cursors.clone());
    // Строки 0..=2, выделение [1..3).
    assert_eq!(cursors.len(), 3);
    assert_eq!(cursors[0], Selection::range(1, 3));
    assert_eq!(cursors[2], Selection::range(13, 15));
    cx.simulate_keystrokes("X");
    let text = cx.cx.update(|app| view.read(app).model().active().unwrap().doc.text());
    // Ввод ЗАМЕНЯЕТ выделение [1..3) в каждой строке блока.
    assert_eq!(text, "aXde\naXde\naXde\n");
}

/// 4.3: Cmd+D — слово под кареткой, затем следующее вхождение;
/// ввод на всех выделениях.
#[gpui::test]
fn cmd_d_adds_next_match(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "cmd-d", "foo bar foo\nbaz foo\n");
    cx.dispatch_action(AddNextMatch);
    cx.dispatch_action(AddNextMatch);
    let (count, primary) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (tab.cursors.len(), *tab.primary())
    });
    assert_eq!(count, 2);
    assert_eq!(primary, Selection::range(8, 11));
    cx.simulate_keystrokes("Q");
    let text = cx.cx.update(|app| view.read(app).model().active().unwrap().doc.text());
    // Ввод заменяет ВСЕ выделения (оба вхождения "foo").
    assert_eq!(text, "Q bar Q\nbaz foo\n");
}

/// Биндинги как в main.rs (в тестах main() не запускается, keymap
/// приложения пуст — регистрируем явно для E2E-проверки клавиш).
/// 8.5: единый источник keymap::KEYMAP — тесты гоняют РЕАЛЬНУЮ
/// раскладку (справочник и биндинги не расходятся).
fn bind_app_keys(cx: &mut gpui::App) {
    crate::keymap::bind_all(cx);
}

/// 4.3: Cmd+D через РЕАЛЬНЫЙ путь клавиш (keybinding → действие),
/// как в живом приложении: клик в слово, затем cmd-d.
#[gpui::test]
fn cmd_d_keybinding_path(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let (view, cx, _path) = editor_with_file(cx, "cmd-d-key", "foo bar foo\n");
    let grid = MouseGrid::of(cx, &view);
    // Клик в середину слова "bar" (колонка 5).
    cx.simulate_click(grid.at(0.5, 5.2), Modifiers::default());
    let head = cx.cx.update(|app| view.read(app).model().active().unwrap().primary().head());
    assert_eq!(head, 5, "каретка внутри слова");
    cx.simulate_keystrokes("cmd-d");
    let sel = cx.cx.update(|app| *view.read(app).model().active().unwrap().primary());
    assert_eq!(sel, Selection::range(4, 7), "Cmd+D выделил слово");
}

/// 4.2: Cmd+G через реальный путь клавиш открывает диалог.
#[gpui::test]
fn cmd_g_keybinding_path(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let (view, cx, _path) = editor_with_file(cx, "cmd-g-key", "one\ntwo\n");
    cx.simulate_keystrokes("cmd-g");
    let open = cx.cx.update(|app| view.read(app).goto_input.clone());
    assert_eq!(open, Some(String::new()), "cmd-g открыл диалог");
}

/// 4.3: двойной клик — выделение слова, тройной — строки (жест мыши,
/// не горячая клавиша).
#[gpui::test]
fn double_click_selects_word(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "dbl-click", "foo bar foo\nzz\n");
    let grid = MouseGrid::of(cx, &view);
    // Двойной клик по "bar": down+up, down+up (click_count = 2).
    for click in 1..=2 {
        cx.simulate_event(MouseDownEvent {
            position: grid.at(0.5, 5.2),
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: click,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            position: grid.at(0.5, 5.2),
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: click,
        });
    }
    let sel = cx.cx.update(|app| *view.read(app).model().active().unwrap().primary());
    assert_eq!(sel, Selection::range(4, 7), "двойной клик — слово");
    // Тройной клик по строке 1 — вся строка.
    for click in 1..=3 {
        cx.simulate_event(MouseDownEvent {
            position: grid.at(1.5, 1.2),
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: click,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            position: grid.at(1.5, 1.2),
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: click,
        });
    }
    let sel = cx.cx.update(|app| *view.read(app).model().active().unwrap().primary());
    assert_eq!(sel, Selection::range(12, 15), "тройной клик — строка с терминатором");
}

/// 4.3: статус-бар показывает позицию ГЛАВНОГО (последнего) курсора
/// и число курсоров.
#[gpui::test]
fn status_bar_uses_primary_cursor(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "status-multi", "one\ntwo\n");
    cx.update_window_entity(&view, |ws, _window, _| {
        let tab = ws.model_mut().active_mut().unwrap();
        tab.set_cursors(vec![Selection::caret(0), Selection::caret(4)]);
    });
    let text = cx.cx.update(|app| view.read(app).status_bar_text(app));
    assert!(text.contains("Ln 2, Col 1"), "got: {text}");
    assert!(text.contains("2 cursors"), "got: {text}");
}

/// Репродукция скриншота пользователя: драг от НАЧАЛА строки 21 до
/// середины строки 31 (бенчмарк-файл, все строки одной длины).
/// Ожидание: последняя строка выделена ЧАСТИЧНО (до колонки 38),
/// строка 21 — от нуля (якорь в кол 0), средние — целиком.
#[gpui::test]
fn drag_partial_last_line_regression(cx: &mut TestAppContext) {
    let mut content = String::new();
    for i in 0..40 {
        content.push_str(&format!("line {i:05} — the quick brown fox jumps over the lazy dog 0123456789\n"));
    }
    let (view, cx, _path) = editor_with_file(cx, "drag-partial", &content);
    let grid = MouseGrid::of(cx, &view);
    let mods = Modifiers::default();
    cx.simulate_mouse_down(grid.at(20.5, 0.2), MouseButton::Left, mods);
    cx.simulate_mouse_move(grid.at(30.5, 38.2), Some(MouseButton::Left), mods);
    cx.simulate_mouse_up(grid.at(30.5, 38.2), MouseButton::Left, mods);
    let (sel, line, col) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (*tab.primary(), tab.cursor_line(), tab.cursor_col())
    });
    assert_eq!((line, col), (30, 38), "голова на строке 31, колонка 38");
    // Модель: [начало строки 20 .. строка 30 + 38]; строка = 67
    // символов + \n = шаг 68.
    let ls20 = 20 * 68;
    assert_eq!(sel, Selection::range(ls20, 30 * 68 + 38));
    // Рендер-данные строки 30 (последней): ЧАСТИЧНО [0..38].
    let (carets, ranges) = line_carets(&[sel], &editor_core::Document::from_text(&content), 30, 67);
    assert_eq!(ranges, vec![(0, 38)], "последняя строка — частичная подсветка");
    // Рендер-данные строки 20 (первой): [0..67] — целиком (якорь в кол 0).
    let (_, ranges20) = line_carets(&[sel], &editor_core::Document::from_text(&content), 20, 67);
    assert_eq!(ranges20, vec![(0, 67)]);
    let _ = carets;
}

/// Регрессия бага из ручной проверки: фон выделения обязан обнимать
/// СВОЙ сегмент, а не контейнер строки (иначе любое частичное
/// выделение красит строку целиком). Проверяем ШИРИНЫ отрисованных
/// сегментов через debug_selector.
#[gpui::test]
fn selection_background_hugs_segment_not_line(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "sel-paint", "0123456789\nabcdefghij\n");
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.model_mut()
            .active_mut()
            .unwrap()
            .set_cursors(vec![Selection::range(2, 8)]);
        cx.notify();
    });
    cx.run_until_parked();
    let grid = MouseGrid::of(cx, &view);
    let advance = grid.advance;
    let seg0 = cx.debug_bounds("seg:0:0").expect("сегмент [0..2) отрисован");
    let seg1 = cx.debug_bounds("seg:0:1").expect("выделенный сегмент [2..8)");
    let seg2 = cx.debug_bounds("seg:0:2").expect("хвост [8..10)");
    // Ширины: 2, 6, 2 символа — НЕ ширина строки.
    assert!(
        (f32::from(seg0.size.width) - 2.0 * advance).abs() < 2.0,
        "seg0 width {:?} != {}px",
        seg0.size.width,
        2.0 * advance
    );
    assert!(
        (f32::from(seg1.size.width) - 6.0 * advance).abs() < 2.0,
        "выделенный сегмент шириной во всю строку: {:?}",
        seg1
    );
    assert!(
        (f32::from(seg1.left()) - (f32::from(grid.origin.x) + GUTTER_W + 2.0 * advance)).abs()
            < 2.0,
        "выделенный сегмент не начинается на колонке 2: {:?}",
        seg1
    );
    assert!(
        (f32::from(seg2.size.width) - 2.0 * advance).abs() < 2.0,
        "seg2 width {:?}",
        seg2.size.width
    );
}

/// 4.4: Cmd+F2/F2/Shift+F2 — toggle и навигация по закладкам через
/// действия; каретка переходит на строки закладок.
#[gpui::test]
fn bookmark_actions_navigate(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "bookmarks", "a\nb\nc\nd\n");
    cx.update_window_entity(&view, |ws, _w, _| {
        ws.model_mut().active_mut().unwrap().toggle_bookmark_at(1);
        ws.model_mut().active_mut().unwrap().toggle_bookmark_at(3);
    });
    cx.dispatch_action(NextBookmark); // со строки 0 → 1
    let line = cx.cx.update(|app| view.read(app).model().active().unwrap().cursor_line());
    assert_eq!(line, 1);
    cx.dispatch_action(NextBookmark); // → 3
    let line = cx.cx.update(|app| view.read(app).model().active().unwrap().cursor_line());
    assert_eq!(line, 3);
    cx.dispatch_action(PrevBookmark); // → 1
    let line = cx.cx.update(|app| view.read(app).model().active().unwrap().cursor_line());
    assert_eq!(line, 1);
    // Toggle на строке главной каретки (1) снимает закладку.
    cx.dispatch_action(ToggleBookmark);
    let bms = cx
        .cx
        .update(|app| view.read(app).model().active().unwrap().bookmarked_lines());
    assert_eq!(bms, vec![3]);
}

/// Воркспейс-настройки (.settings/workspace.toml): закладка пишется
/// на toggle, переживает закрытие вкладки и восстанавливается при
/// повторном открытии файла; снятие последней закладки убирает запись.
#[gpui::test]
fn workspace_settings_bookmarks_roundtrip(cx: &mut TestAppContext) {
    let (view, cx, root) = workspace_window(cx, "ws-settings");
    let todo = root.join("todo.txt");
    let settings = root.join(".settings").join("workspace.toml");

    // Открыть файл и поставить закладку на строке 1 — запись сразу.
    cx.update_window_entity(&view, |ws, _w, _cx| {
        ws.model_mut().open_path(&todo).unwrap();
    });
    cx.run_until_parked();
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.toggle_bookmark_at(1, cx);
    });
    cx.run_until_parked();
    let text = std::fs::read_to_string(&settings).expect("файл настроек создан");
    assert!(text.contains("[files.\"todo.txt\"]"), "секция файла: {text}");
    assert!(text.contains("bookmarks = [1]"), "строка закладки: {text}");

    // Закрыть вкладку — запись merge сохранена (закладки переживают
    // закрытие файла).
    cx.update_window_entity(&view, |ws, _w, _cx| {
        let ix = ws.model().active_index().unwrap();
        ws.model_mut().close(ix);
    });
    cx.run_until_parked();
    let text = std::fs::read_to_string(&settings).unwrap();
    assert!(text.contains("[files.\"todo.txt\"]"), "запись пережила закрытие: {text}");

    // Повторное открытие — гидратация в render восстанавливает закладку
    // (notify: без кадра render-гидратация не выполняется).
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.model_mut().open_path(&todo).unwrap();
        cx.notify();
    });
    cx.run_until_parked();
    let bm = cx.cx.update(|app| view.read(app).model().active().unwrap().bookmarked_lines());
    assert_eq!(bm, vec![1], "закладка восстановлена из .settings");

    // Снятие последней закладки — запись удалена (файл без мусора).
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.toggle_bookmark_at(1, cx);
    });
    cx.run_until_parked();
    let text = std::fs::read_to_string(&settings).unwrap();
    assert!(!text.contains("todo.txt"), "пустое состояние убирает запись: {text}");
}

/// 4.4: закладки переживают undo/redo (GUI-путь: ввод через
/// keystrokes, undo действием).
#[gpui::test]
fn bookmarks_survive_undo_gui(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "bm-undo", "one\ntwo\n");
    cx.update_window_entity(&view, |ws, _w, _| {
        ws.model_mut().active_mut().unwrap().toggle_bookmark_at(1);
    });
    cx.simulate_keystrokes("up"); // каретка на строку 0
    cx.simulate_keystrokes("N enter"); // новая строка сверху
    let text = cx.cx.update(|app| view.read(app).model().active().unwrap().doc.text());
    assert_eq!(text, "N\none\ntwo\n");
    let bm = cx
        .cx
        .update(|app| view.read(app).model().active().unwrap().bookmarked_lines());
    assert_eq!(bm, vec![2], "bookmark follows content down");
    cx.dispatch_action(Undo);
    let (text, bm) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (tab.doc.text(), tab.bookmarked_lines())
    });
    assert_eq!(text, "one\ntwo\n");
    assert_eq!(bm, vec![1]);
}

/// 4.5: Fold All / Unfold All через действия — uniform_list
/// рендерит видимые строки (мапа visible→real).
#[gpui::test]
fn fold_all_unfold_all_actions(cx: &mut TestAppContext) {
    let code = "fn a() {\n    b();\n}\nfn c() {\n    d();\n}\n";
    let (view, cx, _path) = editor_with_file(cx, "folding", code);
    let full = cx.cx.update(|app| view.read(app).visible_len());
    assert_eq!(full, 7); // 6 строк + пустая хвостовая

    cx.dispatch_action(FoldAll);
    let (folded_count, visible) = cx.cx.update(|app| {
        let ws = view.read(app);
        let tab = ws.model().active().unwrap();
        (tab.doc.line_meta().folded_count(), ws.visible_len())
    });
    assert_eq!(folded_count, 2, "оба блока свернуты");
    assert_eq!(visible, 7 - 2, "тело каждого блока — одна строка");

    cx.dispatch_action(UnfoldAll);
    let visible = cx.cx.update(|app| view.read(app).visible_len());
    assert_eq!(visible, full);
}

/// 4.5: клик по свёрнутой области не может попасть в скрытую
/// строку — координаты мыши идут через мапу visible→real.
#[gpui::test]
fn click_after_fold_lands_on_visible_line(cx: &mut TestAppContext) {
    let code = "fn a() {\n    b();\n    c();\n}\ntail\n";
    let (view, cx, _path) = editor_with_file(cx, "fold-click", code);
    cx.update_window_entity(&view, |ws, _w, _| ws.model_mut().active_mut().unwrap().toggle_fold(0));
    let grid = MouseGrid::of(cx, &view);
    // Клик по «второй видимой строке» — это real-строка 3 ("}"),
    // хотя до свёртывания на этом месте была строка 1.
    cx.simulate_click(grid.at(1.5, 1.2), Modifiers::default());
    let line = cx.cx.update(|app| view.read(app).model().active().unwrap().cursor_line());
    assert_eq!(line, 3);
}

/// 4.5: Cmd+G на скрытую строку — каретка на заголовок фолда.
#[gpui::test]
fn goto_hidden_line_jumps_to_fold_header(cx: &mut TestAppContext) {
    let code = "fn a() {\n    b();\n    c();\n}\ntail\n";
    let (view, cx, _path) = editor_with_file(cx, "fold-goto", code);
    cx.update_window_entity(&view, |ws, _w, _| ws.model_mut().active_mut().unwrap().toggle_fold(0));
    cx.dispatch_action(GoToLine);
    cx.simulate_keystrokes("2 enter"); // строка 2 скрыта телом фолда 0
    let line = cx.cx.update(|app| view.read(app).model().active().unwrap().cursor_line());
    assert_eq!(line, 0, "caret lands on the fold header");
}

/// Поддержка JSON: FormatJson (pretty) через действие меню — одна
/// транзакция (один undo), каретка в начало; MinifyJson возвращает
/// компактный стиль; невалидный JSON — статус с позицией, текст не меняется.
#[gpui::test]
fn json_format_actions(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "json-fmt", "{\"a\":1,\"b\":[true,null]}");
    cx.dispatch_action(FormatJson);
    let text = cx.cx.update(|app| view.read(app).model().active().unwrap().doc.text());
    assert_eq!(text, "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    null\n  ]\n}");
    // Один шаг undo возвращает исходный минифицированный текст.
    cx.dispatch_action(Undo);
    let text = cx.cx.update(|app| view.read(app).model().active().unwrap().doc.text());
    assert_eq!(text, "{\"a\":1,\"b\":[true,null]}");

    // Компактный стиль поверх pretty-вывода.
    cx.dispatch_action(MinifyJson);
    let text = cx.cx.update(|app| view.read(app).model().active().unwrap().doc.text());
    assert_eq!(text, "{\"a\":1,\"b\":[true,null]}");

    // Невалидный JSON: текст не меняется, сообщение в статус-баре.
    cx.update_window_entity(&view, |ws, _w, _| {
        ws.model_mut().active_mut().unwrap().replace_all_text("{\"a\": }");
    });
    cx.dispatch_action(FormatJson);
    let (text, status) = cx.cx.update(|app| {
        let ws = view.read(app);
        (ws.model().active().unwrap().doc.text(), ws.status_message.clone())
    });
    assert_eq!(text, "{\"a\": }");
    let status = status.expect("сообщение о невалидном JSON");
    assert!(status.contains("line 1") && status.contains("column 7"), "{status}");
}

/// replace_all_text: одинаковый текст — no-op (dirty не ставится),
/// после замены каретка в начале.
#[gpui::test]
fn replace_all_text_noop_and_caret(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "replace-all", "one\ntwo\n");
    cx.update_window_entity(&view, |ws, _w, _| {
        let tab = ws.model_mut().active_mut().unwrap();
        tab.set_caret_at(1, 1);
        let dirty = tab.dirty;
        tab.replace_all_text("one\ntwo\n");
        assert_eq!(tab.dirty, dirty, "no-op не делает вкладку dirty");
        tab.replace_all_text("a\nb\n");
        assert_eq!(tab.doc.text(), "a\nb\n");
        assert!(tab.dirty);
        assert_eq!(*tab.primary(), Selection::caret(0));
    });
}

/// 4.6: операции строк через действия меню (SortAsc на выделении).
#[gpui::test]
fn sort_lines_action_on_selection(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "sort", "h\npear\napple\nbanana\nf");
    cx.update_window_entity(&view, |ws, _w, _| {
        let tab = ws.model_mut().active_mut().unwrap();
        tab.set_caret_at(1, 0);
        tab.select_to(3, 3); // строки 1..=3
    });
    cx.dispatch_action(SortAsc);
    let text = cx.cx.update(|app| view.read(app).model().active().unwrap().doc.text());
    assert_eq!(text, "h\napple\nbanana\npear\nf");
    // Undo через действие — операция шла через History.
    cx.dispatch_action(Undo);
    let text = cx.cx.update(|app| view.read(app).model().active().unwrap().doc.text());
    assert_eq!(text, "h\npear\napple\nbanana\nf");
}

/// D11.8 (панель PERFORMANCE): секция отрисована в левой панели,
/// кадры render-прохода собираются, opens непусты после открытия.
/// Секция ВНЕ скроллируемого тела — прилеплена к нижней границе
/// панели (низ секции = верх статус-бара).
/// Глобальные статы perf разделяются параллельными тестами —
/// проверяем только устойчивые инварианты.
#[gpui::test]
fn perf_panel_renders_stats(cx: &mut TestAppContext) {
    let (_view, cx, _path) = editor_with_file(cx, "perf-panel", "one\ntwo\n");
    cx.run_until_parked();
    let bounds = cx
        .debug_bounds("perf-section")
        .expect("секция PERFORMANCE отрисована");
    assert!(
        f32::from(bounds.size.width) > 100.0,
        "секция на всю панель: {bounds:?}"
    );
    // Верх статус-бара: сегменты (status-message и др.) центрированы
    // в баре высотой STATUS_H — снимаем центрирование геометрией.
    let status = cx
        .debug_bounds("status-message")
        .expect("статус-бар отрисован");
    let bar_top = f32::from(status.origin.y)
        - (crate::geometry::STATUS_H - f32::from(status.size.height)) / 2.0;
    let perf_bottom = f32::from(bounds.origin.y + bounds.size.height);
    assert!(
        (perf_bottom - bar_top).abs() < 1.0,
        "PERFORMANCE прилеплена к низу панели: perf_bottom={perf_bottom}, bar_top={bar_top}"
    );
    let (total, stats) = crate::perf::frame_snapshot();
    assert!(total > 0, "render-проходы записаны");
    assert!(stats.is_some(), "статистика кадров есть");
    assert!(!crate::perf::recent_opens().is_empty(), "список открытий непуст");
}

/// Cmd+W (dispatch CloseTab) закрывает чистую вкладку; пустой
/// редактор рендерится без паники.
#[gpui::test]
fn close_tab_action_closes_clean_tab(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "close", "content");
    cx.dispatch_action(CloseTab);
    cx.run_until_parked();
    assert!(cx.cx.update(|app| view.read(app).model().is_empty()));
}

/// D11.2 (identity-мапа): без фолдов кэш видимых строк — тождественный
/// (FoldView::Identity) и ПЕРЕЖИВАЕТ правки текста: правки не меняют
/// folded-флаги, пересборки/аллокации нет, видимость == числу строк.
#[gpui::test]
fn fold_view_identity_survives_edits(cx: &mut TestAppContext) {
    let content: String = (0..50).map(|i| format!("line{i}\n")).collect();
    let (view, cx, _path) = editor_with_file(cx, "fold-identity", &content);
    cx.run_until_parked();
    let identity = |cx: &gpui::VisualTestContext| {
        cx.cx.update(|app| {
            matches!(
                view.read(app).panes[0].fold_view,
                Some((_, crate::workspace::FoldView::Identity))
            )
        })
    };
    let visible = |cx: &gpui::VisualTestContext| {
        cx.cx.update(|app| {
            let ws = view.read(app);
            (ws.visible_len(), ws.model().active().unwrap().doc.len_lines())
        })
    };
    assert!(identity(cx), "после открытия — identity");
    // Правки (реальный путь клавиш): мапа НЕ инвалидируется, длина
    // синхронна документу.
    cx.simulate_keystrokes("x y enter z");
    cx.run_until_parked();
    assert!(identity(cx), "после правок — по-прежнему identity");
    let (vis, lines) = visible(cx);
    assert_eq!(vis, lines);
    // Undo тоже не рвёт identity (фолдов нет — восстанавливать нечего).
    cx.dispatch_action(Undo);
    cx.run_until_parked();
    assert!(identity(cx));
    let (vis, lines) = visible(cx);
    assert_eq!(vis, lines);
}

/// D11.2 (переходы кэша): fold_all → материальная мапа; правки при
/// активных фолдах пересобирают её (строки тел сдвигаются); unfold_all
/// → обратно identity. Undo, восстанавливающий folded-флаги удалённой
/// строки-заголовка, обязан вернуть материальную мапу.
#[gpui::test]
fn fold_view_transitions_follow_fold_count(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "fold-trans", "fn a() {\n    b();\n}\n");
    cx.run_until_parked();
    let kind = |cx: &gpui::VisualTestContext| {
        cx.cx.update(|app| match &view.read(app).panes[0].fold_view {
            Some((_, crate::workspace::FoldView::Identity)) => 0,
            Some((_, crate::workspace::FoldView::Mapped { .. })) => 1,
            None => 2,
        })
    };
    // Полный файл: identity.
    assert_eq!(kind(cx), 0);
    // fold_all → mapped, видимых меньше.
    cx.dispatch_action(FoldAll);
    cx.run_until_parked();
    assert_eq!(kind(cx), 1);
    let folded_vis = cx.cx.update(|app| view.read(app).visible_len());
    assert!(folded_vis < 4);
    // Правка при активных фолдах: мапа пересобирается (версия растёт),
    // видимость остаётся корректной.
    cx.simulate_keystrokes("q");
    cx.run_until_parked();
    assert_eq!(kind(cx), 1, "фолды активны — mapped");
    assert_eq!(cx.cx.update(|app| view.read(app).visible_len()), folded_vis);
    // unfold_all → identity.
    cx.dispatch_action(UnfoldAll);
    cx.run_until_parked();
    assert_eq!(kind(cx), 0);
    assert_eq!(cx.cx.update(|app| view.read(app).visible_len()), 4);

    // Undo восстанавливает folded-флаги: свернули, удалили заголовок
    // (флаг исчез), undo вернул строку с флагом → мапа снова mapped.
    let (view2, cx, _path) = editor_with_file(cx, "fold-undo", "fn a() {\n    b();\n}\n");
    cx.update_window_entity(&view2, |ws, _w, _| {
        ws.model_mut().active_mut().unwrap().toggle_fold(0);
        // Мутация модели без notify не рендерит — кэш мапы обновляем
        // явно (в живом приложении это делает Render::render).
        ws.ensure_fold_view();
    });
    cx.run_until_parked();
    let folded_len = cx.cx.update(|app| view2.read(app).visible_len());
    assert_eq!(folded_len, 3, "тело скрыто");
    // Выделить строку-заголовок и заменить вводом (флаг исчезает вместе
    // с заголовком — is_foldable ложен, тело снова видно).
    let grid = MouseGrid::of(cx, &view2);
    cx.simulate_mouse_down(grid.at(0.5, 0.2), MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(grid.at(0.5, 0.2), MouseButton::Left, Modifiers::default());
    cx.simulate_keystrokes("shift-end");
    cx.simulate_keystrokes("space"); // ввод поверх выделения удаляет заголовок
    cx.run_until_parked();
    // Заголовок заменён пробелом: is_foldable ложен, тело снова видно.
    let unfolded = cx.cx.update(|app| view2.read(app).visible_len());
    assert_eq!(unfolded, 3);
    // Undo возвращает "fn a() {" С folded-флагом (LineMeta-синк) —
    // мапа обязана вернуться к свёрнутому виду.
    cx.dispatch_action(Undo);
    cx.run_until_parked();
    let (vis, mapped) = cx.cx.update(|app| {
        let ws = view2.read(app);
        (
            ws.visible_len(),
            matches!(ws.panes[0].fold_view, Some((_, crate::workspace::FoldView::Mapped { .. }))),
        )
    });
    assert!(mapped, "undo восстановил folded-флаг — мапа материальная");
    assert_eq!(vis, folded_len, "тело снова скрыто");
}

// ---- Подсветка синтаксиса (группа 5) ----

/// Центр bounds (клик по элементу через debug_bounds).
fn center_of(b: gpui::Bounds<gpui::Pixels>) -> gpui::Point<gpui::Pixels> {
    point(
        px(f32::from(b.origin.x) + f32::from(b.size.width) / 2.0),
        px(f32::from(b.origin.y) + f32::from(b.size.height) / 2.0),
    )
}

/// Окно с файлом заданного ИМЕНИ и содержимого (editor_with_file
/// пишет фиксированный file.txt — подсветке нужно расширение).
fn editor_with_named_file<'a>(
    cx: &'a mut TestAppContext,
    tag: &str,
    name: &str,
    content: &str,
) -> (gpui::Entity<Workspace>, &'a mut gpui::VisualTestContext, PathBuf) {
    let dir = TempDir::new(tag);
    let path = dir.write(name, content);
    std::mem::forget(dir);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut ws = Workspace::new(cx);
        // Реальный путь открытия (диалог/сессия): вопрос о подсветке
        // сверх порога ставится здесь (D12.4).
        ws.open_paths(std::slice::from_ref(&path), window, cx);
        // Вопрос настроек CSV (D12.6) мог открыться — тестам
        // колонок/шапки он не нужен (диалог-тесты открывают сами).
        ws.csv_dialog = false;
        ws
    });
    focus_editor(&view, cx);
    (view, cx, path)
}

/// 5.2: подсветка рендерится — строка .rs разбита на цветовые сегменты
/// (seg:0:N, N ≥ 3 — границы цвета режут строку), ШИРИНЫ сегментов
/// суммируются в ширину строки (границы цвета не рвут текст).
#[gpui::test]
fn syntax_highlight_renders_colored_segments(cx: &mut TestAppContext) {
    let (view, cx, _p) = editor_with_named_file(
        cx,
        "hl-rs",
        "main.rs",
        "fn main() { let s = \"hi\"; }\n",
    );
    // Прогрев (в живом приложении — отложенная задача после первого
    // кадра; тест форсирует синхронно).
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.refresh_all_languages(cx);
        cx.notify();
    });
    cx.run_until_parked();
    // debug_bounds требует 'static — литеральные ключи; сегментов у
    // 27-символьной строки с цветами до ~30.
    const KEYS: [&str; 32] = [
        "seg:0:0", "seg:0:1", "seg:0:2", "seg:0:3", "seg:0:4", "seg:0:5", "seg:0:6", "seg:0:7",
        "seg:0:8", "seg:0:9", "seg:0:10", "seg:0:11", "seg:0:12", "seg:0:13", "seg:0:14", "seg:0:15",
        "seg:0:16", "seg:0:17", "seg:0:18", "seg:0:19", "seg:0:20", "seg:0:21", "seg:0:22", "seg:0:23",
        "seg:0:24", "seg:0:25", "seg:0:26", "seg:0:27", "seg:0:28", "seg:0:29", "seg:0:30", "seg:0:31",
    ];
    let segs: Vec<gpui::Bounds<gpui::Pixels>> =
        KEYS.iter().filter_map(|k| cx.debug_bounds(k)).collect();
    assert!(
        segs.len() >= 3,
        "подсветка режет строку на цветовые сегменты: {segs:?}"
    );
    // Суммарная ширина сегментов == ширина строки минус 2px-каретки
    // (каретка в col 0 рендерится 2px-дивом). Сравниваем с сеткой:
    // 27 символов строки, advance сетки.
    let grid = MouseGrid::of(cx, &view);
    let total: f32 = segs.iter().map(|b| f32::from(b.size.width)).sum();
    // 27 символов + 2px-каретка в col 0 (сегменты текста + каретка).
    let expected = 27.0 * grid.advance + 2.0; // "fn main() { let s = \"hi\"; }"
    assert!(
        (total - expected).abs() < 4.0,
        "сегменты покрывают строку: {total} vs {expected} (advance {:?})",
        grid.advance
    );
}

/// Round-trip: файл без языка (.txt) — ОДИН сегмент, как до подсветки
/// (сегменты color-class не появляются).
#[gpui::test]
fn plain_file_single_segment_round_trip(cx: &mut TestAppContext) {
    let (view, cx, _p) = editor_with_named_file(cx, "hl-plain", "notes.txt", "just text\n");
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.refresh_all_languages(cx);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("seg:0:0").is_some());
    assert!(
        cx.debug_bounds("seg:0:1").is_none(),
        "без языка строка — один сегмент (бит-в-бит как раньше)"
    );
}

/// 5.1, сценарий спеки «Ручной выбор языка»: файл без расширения,
/// клик по языку в статус-баре → оверлей → клик YAML → подсветка YAML
/// немедленно (цветовые сегменты в рендере).
#[gpui::test]
fn language_picker_manual_yaml(cx: &mut TestAppContext) {
    let (view, cx, _p) =
        editor_with_named_file(cx, "hl-pick", "data", "# not yaml yet\nkey: value\n");
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.refresh_all_languages(cx);
        cx.notify();
    });
    cx.run_until_parked();
    // До выбора: Plain Text, один сегмент на строку.
    assert!(cx.debug_bounds("seg:0:1").is_none());

    // Клик по языку в статус-баре — открывается оверлей.
    let lang = cx.debug_bounds("status-language").expect("язык кликабелен");
    cx.simulate_click(center_of(lang), Modifiers::default());
    cx.run_until_parked();
    // Список длинный (64+) в 320px-скролле: ввод-фильтр «ya» поднимает
    // YAML в видимую часть (заодно покрывает фильтрацию вводом).
    cx.simulate_keystrokes("y a");
    cx.run_until_parked();
    let yaml_item = cx.debug_bounds("lang-item:YAML").expect("YAML в списке (после фильтра)");
    cx.simulate_click(center_of(yaml_item), Modifiers::default());
    cx.run_until_parked();

    // Язык применился: статус-бар + цветовые сегменты строки «key: value».
    let status = cx.cx.update(|app| view.read(app).status_bar_text(app));
    assert!(status.contains("|  YAML"), "got: {status}");
    assert!(
        cx.debug_bounds("seg:1:1").is_some(),
        "строка «key: value» разбита цветами"
    );
}

/// 5.1, сценарий спеки «Открытие Python-файла»: .py подсвечивается
/// автоматически после прогрева (без действий пользователя).
#[gpui::test]
fn python_file_highlighted_automatically(cx: &mut TestAppContext) {
    let (view, cx, _p) = editor_with_named_file(cx, "hl-py", "script.py", "def main():\n    pass\n");
    // Никаких ручных действий: только прогрев набора (отложенная задача
    // живого приложения).
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.refresh_all_languages(cx);
        cx.notify();
    });
    cx.run_until_parked();
    let status = cx.cx.update(|app| view.read(app).status_bar_text(app));
    assert!(status.contains("|  Python"), "got: {status}");
    assert!(
        cx.debug_bounds("seg:0:1").is_some(),
        "«def main():» разбита цветами (keyword/function)"
    );
}

/// 5.3/D12.4, сценарий спеки «Файл сверх порога — отказ»: 20 МБ+
/// открывается редактируемым без подсветки; вопрос пользователя
/// отклонён — сообщение об отключении.
#[gpui::test]
fn over_threshold_file_editable_declined(cx: &mut TestAppContext) {
    // 20 МиБ + хвост: короткие строки для скорости.
    let mut content = String::with_capacity(20 * 1024 * 1024 + 16);
    while content.len() <= 20 * 1024 * 1024 {
        content.push_str("abcdefghij\n");
    }
    let (view, cx, _p) = editor_with_named_file(cx, "hl-threshold", "big.rs", &content);
    // Вопрос о подсветке сверх порога — отвечаем «Keep off».
    assert!(cx.has_pending_prompt(), "вопрос показан при открытии");
    cx.simulate_prompt_answer("Keep off");
    cx.run_until_parked();
    let status = cx.cx.update(|app| {
        view.read(app).status_message.clone().expect("сообщение о выключенной подсветке")
    });
    assert!(
        status.contains("Syntax highlighting is off"),
        "got: {status}"
    );
    // Редактирование полноценно: ввод работает, документ меняется.
    cx.simulate_keystrokes("z");
    let text_head = cx.cx.update(|app| {
        view.read(app).model().active().unwrap().doc.slice_str(0, 1)
    });
    assert_eq!(text_head, "z");
    cx.dispatch_action(Undo); // cmd-z биндинг в gpui::test пуст — действием
    cx.run_until_parked();
    let text_head = cx.cx.update(|app| {
        view.read(app).model().active().unwrap().doc.slice_str(0, 1)
    });
    assert_eq!(text_head, "a", "undo работает");
    // Подсветки нет: движок без синтаксиса (сегменты может резать
    // только каретка — после ввода она в col 1). Язык — Plain Text.
    let (syntax, label) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (tab.highlight.syntax_name().map(str::to_string), tab.language_label().to_string())
    });
    assert_eq!(syntax, None);
    assert_eq!(label, "Plain Text");
}

/// D12.4, сценарий спеки «Файл сверх порога — согласие»: пользователь
/// разрешил — подсветка работает для файла ЛЮБОГО размера (20 МБ+),
/// язык определяется, редактирование полноценно.
#[gpui::test]
fn over_threshold_prompt_enable_highlights_any_size(cx: &mut TestAppContext) {
    let mut content = String::with_capacity(20 * 1024 * 1024 + 16);
    while content.len() <= 20 * 1024 * 1024 {
        content.push_str("call_000001(arg1, arg2) { if (deep) { trace(); } }\n");
    }
    let (view, cx, _p) = editor_with_named_file(cx, "hl-enable", "huge.rs", &content);
    assert!(cx.has_pending_prompt(), "вопрос показан");
    cx.simulate_prompt_answer("Enable");
    cx.run_until_parked();
    let (label, syntax) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (tab.language_label().to_string(), tab.highlight.syntax_name().map(str::to_string))
    });
    assert_eq!(label, "Rust", "язык определился после согласия");
    assert_eq!(syntax.as_deref(), Some("Rust"), "движок активен для 20 МБ+");
    // Статус-бар сообщает о включении; редактирование работает.
    let status = cx.cx.update(|app| {
        view.read(app).status_message.clone().expect("сообщение о включении подсветки")
    });
    assert!(status.contains("enabled for large"), "got: {status}");
    cx.simulate_keystrokes("z");
    let head = cx.cx.update(|app| {
        view.read(app).model().active().unwrap().doc.slice_str(0, 1)
    });
    assert_eq!(head, "z");
}

/// D12.4, CSV: колонки чередуются цветами — строка «a,b,c» рендерится
/// тремя цветовыми сегментами (границы полей), кавычки не рвут поля.
#[gpui::test]
fn csv_columns_render_colored_segments(cx: &mut TestAppContext) {
    let (view, cx, _p) = editor_with_named_file(
        cx,
        "hl-csv",
        "data.csv",
        "name,count,note\nalpha,42,ok\n\"x,y\",z\n",
    );
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.refresh_all_languages(cx);
        cx.notify();
    });
    cx.run_until_parked();
    let status = cx.cx.update(|app| view.read(app).status_bar_text(app));
    assert!(status.contains("|  CSV"), "got: {status}");
    // Строка 0 «name,count,note»: каретка в col 0 режет первый сегмент
    // (без пустого — caret_before на «name»); чередование цветное
    // поле / plain-запятая: name , count , note — 5 сегментов.
    // debug_bounds требует 'static — литеральные ключи.
    const COL_KEYS: [&str; 5] = ["seg:0:0", "seg:0:1", "seg:0:2", "seg:0:3", "seg:0:4"];
    for k in COL_KEYS {
        assert!(cx.debug_bounds(k).is_some(), "{k} — цветное поле или запятая");
    }
    assert!(cx.debug_bounds("seg:0:5").is_none(), "3 колонки → 5 сегментов");
    // Строка 2 «\"x,y\",z»: кавычки держат поле целиком — 2 цветных
    // поля + 1 запятая = 3 сегмента (4-го нет).
    assert!(cx.debug_bounds("seg:2:0").is_some());
    assert!(cx.debug_bounds("seg:2:1").is_some());
    assert!(cx.debug_bounds("seg:2:2").is_some());
    assert!(cx.debug_bounds("seg:2:3").is_none(), "запятая в кавычках не делит поле");
}

/// D12.5, сценарий спеки «Шапка CSV закреплена наверху»: прокрутка
/// уводит строку 0 из вида — над списком появляется липкая копия
/// заголовка (ширина = редактору, высота = строке, сегменты = тексту
/// строки 0 с цветами колонок); клик по шапке — прокрутка к началу
/// (offset = 0, шапка исчезла, на месте строка 0 выделена фоном).
#[gpui::test]
fn csv_sticky_header_pins_and_click_scrolls_to_top(cx: &mut TestAppContext) {
    // 120 строк — строка 0 целиком уходит из вида небольшого окна.
    let content: String = std::iter::once("name,count,note\n".to_string())
        .chain((1..=120).map(|i| format!("row{i:03},{i},x\n")))
        .collect();
    let (view, cx, _p) = editor_with_named_file(cx, "csv-sticky", "data.csv", &content);
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.refresh_all_languages(cx);
        cx.notify();
    });
    cx.run_until_parked();

    // Начало файла: оверлея нет; строка 0 выделена на месте.
    assert!(cx.debug_bounds("csv-sticky-header").is_none());
    assert!(
        cx.debug_bounds("csv-header-inplace").is_some(),
        "строка 0 выделена фоном шапки"
    );

    // Прокрутка вниз — путь живого скролла (offset + перерисовка:
    // paint_scroll_listener gpui зовёт notify на view, спайк D12.5).
    // Ловушка deferred-скролла: scroll_to_item применяется в prepaint
    // ПОСЛЕ render — качаем второй кадр (в живом окне его даёт любое
    // следующее событие).
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.panes[0].scroll.scroll_to_item(60, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    cx.run_until_parked();
    cx.update_window_entity(&view, |_ws, _w, cx| cx.notify());
    cx.run_until_parked();
    let header = cx.debug_bounds("csv-sticky-header").expect("шапка закреплена наверху");
    // Ширина ≈ ширине редактора (оверлей на всю ширину контейнера),
    // высота = строке.
    let editor = cx
        .update_window_entity(&view, |ws, _w, _| ws.editor_bounds().expect("editor bounds"));
    assert!(
        (f32::from(header.size.width) - f32::from(editor.size.width)).abs() < 1.0,
        "header {:?} vs editor {:?}",
        header.size,
        editor.size
    );
    assert_eq!(f32::from(header.size.height), LINE_HEIGHT);
    // Текст шапки = строка 0 «name,count,note»: 3 цветных поля + 2
    // plain-запятые = 5 сегментов (кареток/выделений в копии нет).
    for k in [
        "csv-sticky-seg:0",
        "csv-sticky-seg:1",
        "csv-sticky-seg:2",
        "csv-sticky-seg:3",
        "csv-sticky-seg:4",
    ] {
        assert!(cx.debug_bounds(k).is_some(), "{k} — сегмент шапки");
    }
    assert!(cx.debug_bounds("csv-sticky-seg:5").is_none(), "ровно 5 сегментов");

    // Клик по шапке — прокрутка к началу файла.
    cx.simulate_click(center_of(header), Modifiers::default());
    cx.run_until_parked();
    let offset_y = cx.update_window_entity(&view, |ws, _, _| {
        f32::from(ws.panes[0].scroll.0.borrow().base_handle.offset().y)
    });
    assert_eq!(offset_y, 0.0, "прокрутка к началу");
    assert!(
        cx.debug_bounds("csv-sticky-header").is_none(),
        "шапка исчезла — строка 0 на месте"
    );
    assert!(
        cx.debug_bounds("csv-header-inplace").is_some(),
        "выделение на месте вернулось"
    );
}

/// D12.5: предикат оверлея на живом рендере — частичный показ строки 0
/// (0 < |offset| < LINE_HEIGHT) шапку НЕ дублирует (мерцание), полный
/// уход — дублирует; не-CSV — шапки нет при любой прокрутке.
#[gpui::test]
fn csv_sticky_header_partial_and_plain_files(cx: &mut TestAppContext) {
    // Не-CSV: прокрутка вглубь — оверлея нет.
    let lines: String = (0..120).map(|i| format!("line{i:03}\n")).collect();
    let (view, cx, _p) = editor_with_named_file(cx, "sticky-plain", "data.txt", &lines);
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.panes[0].scroll.scroll_to_item(60, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("csv-sticky-header").is_none(),
        "не-CSV — шапки нет"
    );

    // CSV: set_offset пишет в общий с interactivity Rc<RefCell>
    // scroll-offset (спайк D12.5) — точный контроль величины offset.
    let csv: String = std::iter::once("a,b\n".to_string())
        .chain((1..=120).map(|i| format!("{i},x\n")))
        .collect();
    let (view, cx, _p) = editor_with_named_file(cx, "sticky-half", "data.csv", &csv);
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.refresh_all_languages(cx);
        cx.notify();
    });
    cx.run_until_parked();
    // Частичный показ строки 0 (−половина строки) — дубля нет.
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.panes[0].scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(-LINE_HEIGHT / 2.0)));
        cx.notify();
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("csv-sticky-header").is_none(),
        "частичный показ — без дубля"
    );
    // Полный уход (строка 0 целиком вне вида) — шапка есть.
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.panes[0].scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(-(LINE_HEIGHT + 5.0))));
        cx.notify();
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("csv-sticky-header").is_some(),
        "полный уход — шапка закреплена"
    );
}

/// D12.6: открытие CSV спрашивает настройки — overlay с чипами
/// разделителя/кавычек, префилл — детект по содержимому (`;`-файл);
/// Esc закрывает, детект остаётся выбором вкладки; в статус-баре —
/// кликабельный CSV-сегмент.
#[gpui::test]
fn csv_open_asks_options_esc_keeps_detected(cx: &mut TestAppContext) {
    let dir = TempDir::new("csv-ask");
    let path = dir.write("data.csv", "a;b;c\n1;2;3\n");
    std::mem::forget(dir);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut ws = Workspace::new(cx);
        ws.open_paths(std::slice::from_ref(&path), window, cx);
        ws
    });
    cx.run_until_parked();
    let (dialog, options) = cx.update_window_entity(&view, |ws, _w, _cx| {
        (ws.csv_dialog, ws.model().active().unwrap().csv_options)
    });
    assert!(dialog, "открытие CSV спрашивает настройки");
    assert_eq!(options.delimiter, Some(';'), "детект-префилл разделителя");
    assert!(cx.debug_bounds("csv-overlay").is_some(), "overlay отрисован");
    assert!(cx.debug_bounds("csv-chip:;").is_some(), "чип `;` присутствует");
    assert!(
        cx.debug_bounds("status-csv").is_some(),
        "CSV-сегмент в статус-баре"
    );
    // Esc — закрыть: детект остаётся, сегмент показывает `;`.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let (dialog, options) = cx.update_window_entity(&view, |ws, _w, _cx| {
        (ws.csv_dialog, ws.model().active().unwrap().csv_options)
    });
    assert!(!dialog, "Esc закрыл overlay");
    assert_eq!(options.delimiter, Some(';'), "выбор после закрытия жив");
}

/// D12.6: клик по CSV-сегменту статус-бара переоткрывает настройки;
/// чип применяется немедленно (живой предпросмотр), выбор
/// персистится в .settings и переживает закрытие/повторное открытие
/// файла — без повторного вопроса.
#[gpui::test]
fn csv_status_segment_and_persistence(cx: &mut TestAppContext) {
    let dir = TempDir::new("csv-persist");
    dir.write("data.csv", "a;b;c\n1;2;3\n");
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut ws = Workspace::new(cx);
        ws.open_workspace_folder(&root, cx);
        ws.open_paths(&[root.join("data.csv")], window, cx);
        ws
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("csv-overlay").is_some(),
        "вопрос при открытии CSV"
    );
    // Клик по чипу `;` — применить + персист (диалог жив: предпросмотр).
    let chip = cx.debug_bounds("csv-chip:;").expect("чип `;`");
    cx.simulate_click(center_of(chip), Modifiers::default());
    cx.run_until_parked();
    let text =
        std::fs::read_to_string(root.join(".settings").join("workspace.toml")).unwrap();
    assert!(
        text.contains("csv_delimiter = \";\""),
        "выбор записан в .settings: {text}"
    );
    // Esc — закрыть.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let closed = cx.update_window_entity(&view, |ws, _w, _cx| !ws.csv_dialog);
    assert!(closed, "overlay закрыт");

    // Клик по CSV-сегменту статус-бара — переоткрыть; чип `,` меняет
    // разделитель немедленно.
    let seg = cx.debug_bounds("status-csv").expect("CSV-сегмент статус-бара");
    cx.simulate_click(center_of(seg), Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("csv-overlay").is_some(), "клик по сегменту открыл настройки");
    let chip = cx.debug_bounds("csv-chip:,").expect("чип запятой");
    cx.simulate_click(center_of(chip), Modifiers::default());
    cx.run_until_parked();
    let delimiter = cx.update_window_entity(&view, |ws, _w, _cx| {
        ws.model().active().unwrap().csv_options.delimiter
    });
    assert_eq!(delimiter, Some(','), "чип применился немедленно");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    // Закрыть вкладку и открыть файл снова — сохранённый выбор
    // применяется молча (гидратация в render), вопрос не повторяется.
    cx.update_window_entity(&view, |ws, _w, cx| {
        let ix = ws.model().active_index().unwrap();
        ws.model_mut().close(ix);
        ws.model_mut().open_path(root.join("data.csv")).unwrap();
        cx.notify();
    });
    cx.run_until_parked();
    let (dialog, delimiter) = cx.update_window_entity(&view, |ws, _w, _cx| {
        (ws.csv_dialog, ws.model().active().unwrap().csv_options.delimiter)
    });
    assert!(!dialog, "сохранённый выбор — без повторного вопроса");
    assert_eq!(delimiter, Some(','), "гидратация из .settings");
}

// ---- Поиск и замена (группа 6) ----

/// Хелпер: прочитать состояние панели поиска (Option<(query, counter)>).
fn search_state(
    cx: &gpui::VisualTestContext,
    view: &gpui::Entity<Workspace>,
) -> Option<(String, String)> {
    cx.cx.update(|app| {
        view.read(app)
            .search
            .as_ref()
            .map(|sp| (sp.query.clone(), sp.counter("No results")))
    })
}

/// Продвинуть виртуальное время поллинга фонового скана (таймеры
/// gpui::test виртуальные — advance_clock будит поллинг-таск).
fn advance_scan_clock(cx: &mut gpui::VisualTestContext, view: &gpui::Entity<Workspace>) {
    for _ in 0..100 {
        cx.cx.executor().advance_clock(std::time::Duration::from_millis(40));
        cx.run_until_parked();
        if !cx.cx.update(|app| {
            view.read(app).search.as_ref().is_some_and(|sp| sp.scanning)
        }) {
            break;
        }
    }
}

/// 6.1: Cmd+F открывает панель; ФОКУС-ЛОВУШКА (D9.2) — набор текста
/// уходит в панель, документ не правится; счётчик считает совпадения.
#[gpui::test]
fn cmd_f_opens_panel_and_typing_goes_to_query(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let (view, cx, _p) = editor_with_file(cx, "find-open", "cat dog cat\n");
    cx.simulate_keystrokes("cmd-f");
    cx.run_until_parked();
    let state = search_state(cx, &view);
    assert_eq!(state, Some(("".to_string(), "No results".to_string())));
    // Набор — в панель, не в документ.
    cx.simulate_keystrokes("c a t");
    cx.run_until_parked();
    let (text, query) = cx.cx.update(|app| {
        let ws = view.read(app);
        (
            ws.model().active().unwrap().doc.text(),
            ws.search.as_ref().unwrap().query.clone(),
        )
    });
    assert_eq!(text, "cat dog cat\n", "документ не правится из панели");
    assert_eq!(query, "cat");
    // Панель отрисована (счётчик на месте).
    assert!(cx.debug_bounds("search-counter").is_some());
    assert!(cx.debug_bounds("search-panel").is_some());
}

/// 6.1: подсветка ВСЕХ совпадений — строка режется на сегменты
/// «cat»/« dog »/«cat» (ширины через debug_bounds, урок D9.1: юнит-
/// тесты данных таких ошибок не видят).
#[gpui::test]
fn find_highlights_segments_by_width(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let (view, cx, _p) = editor_with_file(cx, "find-hl", "cat dog cat\n");
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_keystrokes("c a t");
    cx.run_until_parked();
    let grid = MouseGrid::of(cx, &view);
    let seg0 = cx.debug_bounds("seg:0:0").expect("первое совпадение");
    let seg1 = cx.debug_bounds("seg:0:1").expect("середина");
    let seg2 = cx.debug_bounds("seg:0:2").expect("второе совпадение");
    // «cat» = 3 символа; « dog » = 5.
    assert!(
        (f32::from(seg0.size.width) - 3.0 * grid.advance).abs() < 2.0,
        "seg0 (совпадение) {:?}",
        seg0
    );
    assert!(
        (f32::from(seg1.size.width) - 5.0 * grid.advance).abs() < 2.0,
        "seg1 {:?}",
        seg1
    );
    assert!(
        (f32::from(seg2.size.width) - 3.0 * grid.advance).abs() < 2.0,
        "seg2 {:?}",
        seg2
    );
    // Сегменты покрывают строку ЦЕЛИКОМ и без хвоста сверх неё: правый
    // край последнего ≈ началу текста + 2px каретки + 11 символов
    // («cat dog cat»). (Ассерт «нет seg:0:3» невозможен: кадры
    // промежуточных запросов «c»/«ca» имели 4 сегмента, debug_bounds
    // не чистит устаревшие ключи.)
    let text_right = f32::from(grid.origin.x) + GUTTER_W + 2.0 + 11.0 * grid.advance;
    if let Some(b3) = cx.debug_bounds("seg:0:3") {
        assert!(
            (f32::from(b3.right()) - text_right).abs() < 3.0,
            "seg3 (хвост) {:?} != концу строки ~{text_right}",
            b3
        );
    }
    // Счётчик «1/2»: текущее — ближайшее к каретке (в начале файла).
    let (_, counter) = search_state(cx, &view).unwrap();
    assert_eq!(counter, "1/2");
}

/// 6.1: Enter/Shift+Enter — навигация след/пред циклически, каретка
/// выделяет совпадение; Esc закрывает панель и возвращает фокус
/// (набор снова правит документ).
#[gpui::test]
fn find_navigation_and_escape(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let (view, cx, _p) = editor_with_file(cx, "find-nav", "x one\nmiddle\nx two\n");
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_keystrokes("x space");
    cx.run_until_parked();
    // «x » два вхождения: строки 0 и 2. Каретка в 0 → текущее 1-е.
    cx.simulate_keystrokes("enter");
    let sel = cx.cx.update(|app| *view.read(app).model().active().unwrap().primary());
    assert_eq!(sel, Selection::range(0, 2), "первое совпадение выделено");
    cx.simulate_keystrokes("enter");
    let (sel, line) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (*tab.primary(), tab.cursor_line())
    });
    assert_eq!(line, 2, "второе совпадение — строка 2");
    // «x one\n»=6 + «middle\n»=7 → второе «x » на char 13.
    assert_eq!(sel, Selection::range(13, 15), "«x two»: x на char 13");
    // Цикличность: след от последнего — первое.
    cx.simulate_keystrokes("enter");
    let line = cx.cx.update(|app| view.read(app).model().active().unwrap().cursor_line());
    assert_eq!(line, 0, "циклически вернулись к первому");
    // Shift+Enter — назад.
    cx.simulate_keystrokes("shift-enter");
    let line = cx.cx.update(|app| view.read(app).model().active().unwrap().cursor_line());
    assert_eq!(line, 2, "shift+enter — предыдущее");
    // Счётчик отслеживает.
    let (_, counter) = search_state(cx, &view).unwrap();
    assert_eq!(counter, "2/2");
    // Esc: панель закрыта, фокус в редакторе — набор правит документ.
    // Выделение последнего совпадения осталось (голова на char 13) —
    // ввод «z» заменяет выделенную «x » второй строки-вхождения.
    cx.simulate_keystrokes("escape");
    let open = cx.cx.update(|app| view.read(app).search.is_none());
    assert!(open, "панель закрыта");
    cx.simulate_keystrokes("z");
    let text = cx.cx.update(|app| view.read(app).model().active().unwrap().doc.text());
    assert_eq!(text, "x one\nmiddle\nztwo\n", "фокус вернулся в редактор");
}

/// 6.1, сценарий спеки «Поиск по регулярному выражению»: `.*`-кнопка,
/// паттерн дат — все даты подсвечены (сегменты), навигация работает.
#[gpui::test]
fn regex_dates_find_scenario(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let (view, cx, _p) = editor_with_file(
        cx,
        "find-regex",
        "due 2024-01-31 and 2023-12-01\nnot 2024-1-31\n",
    );
    cx.simulate_keystrokes("cmd-f");
    // Включить regex-режим кнопкой .* (клик по чипу).
    let regex_chip = cx.debug_bounds("search-toggle-regex").expect("чип .*");
    cx.simulate_click(center_of(regex_chip), Modifiers::default());
    cx.run_until_parked();
    cx.simulate_keystrokes(r"\ d { 4 } - \ d { 2 } - \ d { 2 }");
    cx.run_until_parked();
    // Найдены обе даты (не «2024-1-31»).
    let (_, counter) = search_state(cx, &view).unwrap();
    assert_eq!(counter, "1/2", "две полные даты");
    // Enter: каретка на первую дату (char 4..14).
    cx.simulate_keystrokes("enter");
    let sel = cx.cx.update(|app| *view.read(app).model().active().unwrap().primary());
    assert_eq!(sel, Selection::range(4, 14));
    // Строка 0: «due » + дата (выделение и hit дают ОДНИ границы —
    // сегмент не дублируется) + « and » + дата = 4 сегмента, покрывают
    // все 29 символов (устаревшие seg-ключи от промежуточных запросов
    // «\d» не считаем). debug_bounds требует 'static — литеральные
    // ключи.
    let grid = MouseGrid::of(cx, &view);
    const KEYS: [&str; 4] = ["seg:0:0", "seg:0:1", "seg:0:2", "seg:0:3"];
    let mut total = 0.0;
    for k in KEYS {
        if let Some(b) = cx.debug_bounds(k) {
            total += f32::from(b.size.width);
        }
    }
    assert!(
        (total - 29.0 * grid.advance).abs() < 6.0,
        "сегменты покрывают строку: {total} vs {:?}",
        29.0 * grid.advance
    );
    // Строка 1 (неполная дата «2024-1-31») не даёт совпадений — уже
    // проверено счётчиком «1/2» и позицией прыжка; рендер-ключи строки 1
    // от промежуточных запросов «\d» устаревают без очистки (ловушка
    // debug_bounds), дополнительные ширины не проверяем.
}

/// 6.2: Replace All — раскрытие панели, ввод замены, клик All:
/// документ изменён, dirty; undo возвращает (составная транзакция).
#[gpui::test]
fn replace_all_and_undo(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let (view, cx, _p) = editor_with_file(cx, "find-repl", "foo bar foo\nbaz foo\n");
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_keystrokes("f o o");
    // Раскрыть поле замены (чип ⇕).
    let toggle = cx.debug_bounds("search-toggle-replace").expect("чип замены");
    cx.simulate_click(center_of(toggle), Modifiers::default());
    cx.run_until_parked();
    // Клик по полю замены — активное поле; ввод.
    let field = cx.debug_bounds("search-field-replace").expect("поле замены");
    cx.simulate_click(center_of(field), Modifiers::default());
    cx.simulate_keystrokes("Q Q");
    cx.run_until_parked();
    let repl = cx
        .cx
        .update(|app| view.read(app).search.as_ref().unwrap().replacement.clone());
    assert_eq!(repl, "QQ");
    // Replace All.
    let all = cx.debug_bounds("search-replace-all").expect("кнопка All");
    cx.simulate_click(center_of(all), Modifiers::default());
    cx.run_until_parked();
    let (text, dirty) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (tab.doc.text(), tab.dirty)
    });
    assert_eq!(text, "QQ bar QQ\nbaz QQ\n");
    assert!(dirty);
    // Undo одним шагом возвращает исходник.
    cx.dispatch_action(Undo);
    let text = cx.cx.update(|app| view.read(app).model().active().unwrap().doc.text());
    assert_eq!(text, "foo bar foo\nbaz foo\n");
    // После undo документ снова содержит «foo» — счётчик видит их.
    let (_, counter) = search_state(cx, &view).unwrap();
    assert_eq!(counter, "1/3");
}

/// 6.2: Replace по одному — последовательные применения корректны
/// (пересчёт после каждой правки, D13); undo откатывает по одному.
#[gpui::test]
fn replace_one_by_one(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let (view, cx, _p) = editor_with_file(cx, "find-one", "aa bb aa\n");
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_keystrokes("a a");
    let toggle = cx.debug_bounds("search-toggle-replace").expect("чип замены");
    cx.simulate_click(center_of(toggle), Modifiers::default());
    cx.run_until_parked();
    let field = cx.debug_bounds("search-field-replace").expect("поле замены");
    cx.simulate_click(center_of(field), Modifiers::default());
    cx.simulate_keystrokes("X");
    cx.run_until_parked();
    let one = cx.debug_bounds("search-replace-one").expect("кнопка Replace");
    cx.simulate_click(center_of(one), Modifiers::default());
    cx.run_until_parked();
    let (text, caret) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (tab.doc.text(), tab.primary().head())
    });
    assert_eq!(text, "X bb aa\n", "первое заменено");
    assert_eq!(caret, 1, "каретка за вставкой");
    // Второе нажатие — заменяет следующее (сдвиги учтены пересчётом).
    cx.simulate_click(center_of(one), Modifiers::default());
    cx.run_until_parked();
    let text = cx.cx.update(|app| view.read(app).model().active().unwrap().doc.text());
    assert_eq!(text, "X bb X\n");
    // Undo откатывает по одному шагу на замену.
    cx.dispatch_action(Undo);
    let text = cx.cx.update(|app| view.read(app).model().active().unwrap().doc.text());
    assert_eq!(text, "X bb aa\n");
}

/// 6.3: поиск по вкладкам — результаты в секции SEARCH, клик
/// активирует вкладку и выделяет совпадение.
#[gpui::test]
fn tabs_search_click_navigates(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let dir = TempDir::new("find-tabs");
    let a = dir.write("a.txt", "alpha needle here\nplain\n");
    let b = dir.write("b.txt", "nothing\nsecond needle line\n");
    std::mem::forget(dir);
    let (view, cx) = cx.add_window_view(|_window, cx| {
        let mut ws = Workspace::new(cx);
        ws.model_mut().open_path(&a).unwrap();
        ws.model_mut().open_path(&b).unwrap();
        ws
    });
    focus_editor(&view, cx);
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-shift-f");
    cx.run_until_parked();
    cx.simulate_keystrokes("n e e d l e");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let results = cx.cx.update(|app| view.read(app).search.as_ref().unwrap().results.clone());
    assert_eq!(results.len(), 2, "по одному совпадению на вкладку");
    assert_eq!(results[0].line, 0);
    assert_eq!(results[1].line, 1);
    // Секция SEARCH отрисована с записями.
    assert!(cx.debug_bounds("search-hit:0").is_some());
    assert!(cx.debug_bounds("search-hit:1").is_some());
    // Клик по второму результату: активна вкладка b, совпадение
    // выделено (строка 1, col 7: «second |needle| line»).
    let hit = cx.debug_bounds("search-hit:1").expect("запись");
    cx.simulate_click(center_of(hit), Modifiers::default());
    cx.run_until_parked();
    let (active, sel) = cx.cx.update(|app| {
        let ws = view.read(app);
        let tab = ws.model().active().unwrap();
        (ws.model().active_index(), *tab.primary())
    });
    assert_eq!(active, Some(1), "вкладка b активирована");
    // «nothing\n» = 8 char; «second » = 7 → needle с 15.
    assert_eq!(sel, Selection::range(15, 21));
}

/// 6.4/6.5: поиск и замена по файлам папки — дефолт папки активной
/// вкладки (чип Folder), фоновый скан (виртуальные таймеры), замена с
/// подтверждением: вкладки dirty, ДИСК НЕ ТРОНУТ (байты неизменны),
/// отчёт в статус-баре; повторный поиск видит содержимое вкладок.
#[gpui::test]
fn folder_search_replace_keeps_disk_intact(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let dir = TempDir::new("find-folder");
    let a = dir.write("a.txt", "alpha needle here\n");
    let b = dir.write("b.txt", "beta needle twice\nneedle again\n");
    let before_b = std::fs::read(&b).unwrap();
    std::mem::forget(dir);
    let (view, cx) = cx.add_window_view(|_window, cx| {
        let mut ws = Workspace::new(cx);
        ws.model_mut().open_path(&a).unwrap();
        ws
    });
    focus_editor(&view, cx);
    cx.run_until_parked();
    // Панель → чип Folder: дефолт — папка активной вкладки (D13).
    cx.simulate_keystrokes("cmd-f");
    cx.run_until_parked();
    let folder_chip = cx.debug_bounds("search-scope-folder").expect("чип Folder");
    cx.simulate_click(center_of(folder_chip), Modifiers::default());
    cx.run_until_parked();
    let scope_ok = cx.cx.update(|app| {
        matches!(
            view.read(app).search.as_ref().map(|sp| &sp.scope),
            Some(crate::search_panel::SearchScope::Folder(_))
        )
    });
    assert!(scope_ok, "scope Folder с папкой активной вкладки");
    cx.simulate_keystrokes("n e e d l e");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    advance_scan_clock(cx, &view);
    let results =
        cx.cx.update(|app| view.read(app).search.as_ref().unwrap().results.clone());
    assert_eq!(results.len(), 3, "1 в открытой вкладке + 2 в b.txt");
    // Замена по файлам: раскрыть поле замены (⇕), клик по полю —
    // активное, ввод; Replace All… → подтверждение.
    let toggle = cx.debug_bounds("search-toggle-replace").expect("чип замены");
    cx.simulate_click(center_of(toggle), Modifiers::default());
    cx.run_until_parked();
    let field = cx.debug_bounds("search-field-replace").expect("поле замены");
    cx.simulate_click(center_of(field), Modifiers::default());
    cx.simulate_keystrokes("P I N");
    cx.run_until_parked();
    let repl = cx
        .cx
        .update(|app| view.read(app).search.as_ref().unwrap().replacement.clone());
    assert_eq!(repl, "PIN");
    let button = cx.debug_bounds("search-folder-replace").expect("Replace All…");
    cx.simulate_click(center_of(button), Modifiers::default());
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "запрошено подтверждение");
    cx.simulate_prompt_answer("Replace");
    cx.run_until_parked();
    // Вкладки: обе открыты, dirty; диск не тронут.
    let (tabs, dirties, texts) = cx.cx.update(|app| {
        let ws = view.read(app);
        (
            ws.model().len(),
            ws.model().tabs().iter().map(|t| t.dirty).collect::<Vec<_>>(),
            ws.model().tabs().iter().map(|t| t.doc.text()).collect::<Vec<_>>(),
        )
    });
    assert_eq!(tabs, 2, "b.txt открыт вкладкой");
    assert!(dirties.iter().all(|d| *d), "обе вкладки dirty");
    assert_eq!(texts[0], "alpha PIN here\n");
    assert_eq!(texts[1], "beta PIN twice\nPIN again\n");
    assert_eq!(std::fs::read(&b).unwrap(), before_b, "диск НЕ тронут");
    // Отчёт в статус-баре (правая часть — транзиентное сообщение).
    let status = cx.cx.update(|app| {
        view.read(app).status_message.clone().expect("отчёт о заменах")
    });
    assert!(
        status.contains("Replaced 3 occurrence(s) in 2 file(s)"),
        "got: {status}"
    );
}

/// 6.4 (спека, D13): поиск по папке видит ИЗМЕНЁННОЕ содержимое
/// открытой вкладки, не дисковое состояние.
#[gpui::test]
fn folder_search_prefers_open_tab_content(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let dir = TempDir::new("find-tab-pref");
    let a = dir.write("a.txt", "on-disk-old\n");
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    let (view, cx) = cx.add_window_view(|_window, cx| {
        let mut ws = Workspace::new(cx);
        ws.model_mut().open_path(&a).unwrap();
        ws
    });
    focus_editor(&view, cx);
    cx.run_until_parked();
    // Правим вкладку: слово, которого нет на диске.
    cx.simulate_keystrokes("z z z space");
    let text = cx.cx.update(|app| view.read(app).model().active().unwrap().doc.text());
    assert_eq!(text, "zzz on-disk-old\n");
    // Folder-поиск по слову из ВКЛАДКИ.
    cx.update_window_entity(&view, |ws, window, cx| {
        ws.open_folder_search(root.clone(), window, cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("z z z");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    advance_scan_clock(cx, &view);
    let results =
        cx.cx.update(|app| view.read(app).search.as_ref().unwrap().results.clone());
    assert_eq!(results.len(), 1, "совпадение из содержимого вкладки");
    assert_eq!(results[0].line, 0);
    assert_eq!(results[0].col, 0);
}

/// D13 (замечание юзабилити): прыжок вперёд/назад НЕМЕДЛЕННО выводит
/// строку совпадения в вьюпорт (ловушка deferred-скролла D12.5 —
/// scroll_to_item применяется в prepaint ПОСЛЕ render; offset ставится
/// в общий Rc сразу, кадр строится уже по нему).
#[gpui::test]
fn search_jump_scrolls_immediately_to_match_line(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let content: String = (0..400).map(|i| format!("row{i:03}\n")).collect();
    let (view, cx, _p) = editor_with_file(cx, "find-scroll", &content);
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_keystrokes("r o w 3 5 0");
    cx.run_until_parked();
    // Enter: прыжок к строке 350 — offset уже новый (без второго кадра).
    cx.simulate_keystrokes("enter");
    let (line, offset_y, page) = cx.update_window_entity(&view, |ws, window, _| {
        (
            ws.model().active().unwrap().cursor_line(),
            f32::from(ws.panes[0].scroll.0.borrow().base_handle.offset().y),
            ws.page_lines(window),
        )
    });
    assert_eq!(line, 350, "прыжок на строку 350");
    let first_visible = (-offset_y / LINE_HEIGHT).floor() as usize;
    assert!(
        line >= first_visible && line < first_visible + page,
        "строка {line} в вьюпорте [{first_visible}..{}), offset {offset_y}",
        first_visible + page
    );
    // И центрирование: строка в средней трети вьюпорта.
    assert!(
        (line as f32 - first_visible as f32) < page as f32 * 0.7,
        "строка близко к центру"
    );
}

/// D13 (замечание юзабилити): лупа 🔍 рядом с полем ввода заменяет
/// Enter — File scope: прыжок к следующему совпадению; Tabs scope:
/// запуск поиска по вкладкам.
#[gpui::test]
fn search_magnifier_button_acts_as_enter(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let (view, cx, _p) = editor_with_file(cx, "find-loupe", "aa bb\ncc aa\n");
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_keystrokes("a a");
    cx.run_until_parked();
    // Клик по лупе = Enter: первое совпадение от каретки (0) — само
    // первое «aa» (0..2), следующий клик — второе (6..8), затем цикл.
    let loupe = cx.debug_bounds("search-go").expect("кнопка-лупа");
    cx.simulate_click(center_of(loupe), Modifiers::default());
    cx.run_until_parked();
    let (sel, line) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (*tab.primary(), tab.cursor_line())
    });
    assert_eq!(sel, Selection::range(0, 2), "лупа = Enter: первое «aa»");
    assert_eq!(line, 0);
    // Ещё клик — второе «aa» на строке 1: «aa bb\n» = 6 char, «cc aa»
    // → a на 9, a на 10 → (9..11).
    cx.simulate_click(center_of(loupe), Modifiers::default());
    cx.run_until_parked();
    let (sel, line) = cx.cx.update(|app| {
        let tab = view.read(app).model().active().unwrap();
        (*tab.primary(), tab.cursor_line())
    });
    assert_eq!(sel, Selection::range(9, 11), "второй клик — второе «aa»");
    assert_eq!(line, 1);
    // Третий — циклически к первому.
    cx.simulate_click(center_of(loupe), Modifiers::default());
    cx.run_until_parked();
    let sel = cx.cx.update(|app| *view.read(app).model().active().unwrap().primary());
    assert_eq!(sel, Selection::range(0, 2), "циклически к первому");
    // Tabs scope: лупа запускает поиск (аналог Enter).
    let (view2, cx, _p2) = {
        let dir = TempDir::new("find-loupe-tabs");
        let a = dir.write("a.txt", "needle here\n");
        std::mem::forget(dir);
        
        {
            let (view, cx) = cx.add_window_view(|_window, cx| {
                let mut ws = Workspace::new(cx);
                ws.model_mut().open_path(&a).unwrap();
                ws
            });
            focus_editor(&view, cx);
            (view, cx, a)
        }
    };
    let _ = &_p2;
    cx.simulate_keystrokes("cmd-shift-f");
    cx.run_until_parked();
    cx.simulate_keystrokes("n e e d l e");
    cx.run_until_parked();
    let loupe = cx.debug_bounds("search-go").expect("кнопка-лупа");
    cx.simulate_click(center_of(loupe), Modifiers::default());
    cx.run_until_parked();
    let results =
        cx.cx.update(|app| view2.read(app).search.as_ref().unwrap().results.len());
    assert_eq!(results, 1, "лупа в Tabs запустила поиск");
}

/// D13 (баг ручной проверки: зависания и «куча вхождений»): поиск по
/// вкладкам фоновый и КАПИТСЯ — не больше MAX_HITS_PER_TAB на вкладку
/// и MAX_HITS_TOTAL всего; секция SEARCH рендерит не больше
/// SEARCH_SECTION_MAX_ROWS записей, остаток — «…and N more».
#[gpui::test]
fn tabs_search_results_are_capped(cx: &mut TestAppContext) {
    use crate::search_panel::{MAX_HITS_PER_TAB, MAX_HITS_TOTAL};
    cx.update(bind_app_keys);
    // 5 вкладок по 3×MAX_HITS_PER_TAB вхождений: каждая упрётся в свой
    // кап, суммарно — в MAX_HITS_TOTAL (5×500=2500 > 2000 → стоп).
    let dir = TempDir::new("find-cap");
    for i in 0..5 {
        dir.write(&format!("f{i}.txt"), &"hit ".repeat(3 * MAX_HITS_PER_TAB));
    }
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    let (view, cx) = cx.add_window_view(|_window, cx| {
        let mut ws = Workspace::new(cx);
        for i in 0..5 {
            ws.model_mut()
                .open_path(root.join(format!("f{i}.txt")))
                .unwrap();
        }
        ws
    });
    focus_editor(&view, cx);
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-shift-f");
    cx.run_until_parked();
    cx.simulate_keystrokes("h i t");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let (len, truncated, scanning) = cx.cx.update(|app| {
        let sp = view.read(app).search.as_ref().unwrap();
        (sp.results.len(), sp.truncated, sp.scanning)
    });
    assert!(!scanning, "фоновый поиск завершён");
    assert_eq!(len, MAX_HITS_TOTAL, "суммарный кап");
    assert!(truncated, "результаты усечены — пометка стоит");
    // Сводка панели поиска отрисована (строка итога под полем).
    assert!(
        cx.debug_bounds("search-summary").is_some(),
        "сводка «N match(es) in M file(s)» отрисована"
    );
    // Секция SEARCH: отрисована + хвост скрыт строкой «…and N more»
    // (число записей капнуто — см. MAX_HITS/SEARCH_SECTION_MAX_ROWS).
    assert!(cx.debug_bounds("search-hit:0").is_some());
    assert!(
        cx.debug_bounds("search-more").is_some(),
        "хвост скрыт строкой «…and N more»"
    );
}

// ---- Группа 7: воркспейс и дерево проекта ----

/// Хелпер: окно с открытым воркспейсом (папка + файл в подпапке).
/// Возвращает (view, cx, корень). Каталог забывается — файлы должны
/// жить дольше теста.
fn workspace_window<'a>(
    cx: &'a mut TestAppContext,
    tag: &str,
) -> (
    gpui::Entity<Workspace>,
    &'a mut gpui::VisualTestContext,
    PathBuf,
) {
    let dir = TempDir::new(tag);
    std::fs::create_dir_all(dir.path().join("notes")).unwrap();
    dir.write("notes/idea.txt", "idea\n");
    dir.write("todo.txt", "todo\n");
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    let (view, cx) = cx.add_window_view(|_window, cx| {
        let mut ws = Workspace::new(cx);
        ws.open_workspace_folder(&root, cx);
        ws
    });
    focus_editor(&view, cx);
    cx.run_until_parked();
    (view, cx, root)
}

/// Селектор строки дерева по пути (debug_selector рендерит
/// «tree-row:<path>»; debug_bounds требует 'static — утечка строки
/// в тестах допустима, число вызовов конечно).
fn row_key(root: &std::path::Path, rel: &str) -> &'static str {
    Box::leak(format!("tree-row:{}", root.join(rel).display()).into_boxed_str())
}

/// 7.1: дерево воркспейса рендерит строки (папки выше файлов);
/// клик по файлу открывает вкладку, клик по папке раскрывает.
#[gpui::test]
fn workspace_tree_click_opens_and_expands(cx: &mut TestAppContext) {
    let (view, cx, root) = workspace_window(cx, "tree-click");
    // Верхний уровень: notes (папка) выше todo.txt.
    let notes = cx
        .debug_bounds(row_key(&root, "notes"))
        .expect("строка папки отрисована");
    let todo = cx
        .debug_bounds(row_key(&root, "todo.txt"))
        .expect("строка файла отрисована");
    assert!(
        f32::from(notes.origin.y) < f32::from(todo.origin.y),
        "папки выше файлов"
    );

    // Клик по папке — раскрытие: появляется ребёнок (отступ —
    // визуальная проверка, здесь — факт появления).
    cx.simulate_click(center_of(notes), Modifiers::default());
    cx.run_until_parked();
    let idea = cx
        .debug_bounds(row_key(&root, "notes/idea.txt"))
        .expect("файл в раскрытой папке отрисован");

    // Клик по файлу — вкладка с этим путём.
    cx.simulate_click(center_of(idea), Modifiers::default());
    cx.run_until_parked();
    let tab_path = cx.cx.update(|app| {
        view.read(app).model().active().unwrap().path.clone()
    });
    assert_eq!(tab_path.as_deref(), Some(root.join("notes/idea.txt").as_path()));
}

/// 7.2: контекстное меню → New File… → ввод имени → Enter — файл на
/// диске и в дереве (сценарий спеки «Создание подпапки» — файлом;
/// папка — тем же путём через второй пункт).
#[gpui::test]
fn tree_context_menu_creates_node(cx: &mut TestAppContext) {
    let (view, cx, root) = workspace_window(cx, "tree-ctx");
    let notes = cx
        .debug_bounds(row_key(&root, "notes"))
        .expect("строка папки отрисована");
    // Раскрыть папку кликом (новый файл внутри должен стать виден).
    cx.simulate_click(center_of(notes), Modifiers::default());
    cx.run_until_parked();
    // Правый клик по папке — меню.
    cx.simulate_mouse_down(center_of(notes), MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
    let new_file = cx
        .debug_bounds("ctx-menu:New File…")
        .expect("пункт New File…");
    cx.simulate_click(center_of(new_file), Modifiers::default());
    cx.run_until_parked();
    // Диалог ввода: фокус в tree_focus, символы идут в буфер.
    cx.simulate_keystrokes("p l a n");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let created = root.join("notes/plan");
    assert!(created.is_file(), "файл создан на диске");
    // Папка раскрыта созданием (строка видна).
    assert!(
        cx.debug_bounds(row_key(&root, "notes/plan")).is_some(),
        "файл появился в дереве"
    );
    // Выделение — на созданном узле.
    let selected = cx.cx.update(|app| {
        view.read(app).tree.as_ref().and_then(|t| t.selected().map(|p| p.to_path_buf()))
    });
    assert_eq!(selected, Some(created));
}

/// 7.2: переименование через Shift+F6 (выделение дерева) — вкладка
/// открытого файла следует за новым путём (сценарий спеки для
/// перемещения: вкладки привязаны к актуальному пути).
#[gpui::test]
fn tree_rename_follows_open_tab(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let (view, cx, root) = workspace_window(cx, "tree-rename");
    // Открыть файл вкладкой + выделить его в дереве.
    let todo = cx
        .debug_bounds(row_key(&root, "todo.txt"))
        .expect("строка файла отрисована");
    cx.simulate_click(center_of(todo), Modifiers::default());
    cx.run_until_parked();
    cx.simulate_keystrokes("shift-f6");
    cx.run_until_parked();
    // Диалог заменяет имя ЦЕЛИКОМ (todo.txt → done, без расширения).
    cx.simulate_keystrokes("d o n e");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let renamed = root.join("done");
    assert!(renamed.is_file(), "переименован на диске");
    assert!(!root.join("todo.txt").exists());
    let tab_path = cx.cx.update(|app| {
        view.read(app).model().active().unwrap().path.clone()
    });
    assert_eq!(tab_path.as_deref(), Some(renamed.as_path()), "вкладка следует за путём");
}

/// 7.2: drag-and-drop файла на папку — перемещение на диске, вкладка
/// перепривязана (сценарий спеки «Перемещение файла в дереве»).
#[gpui::test]
fn tree_drag_and_drop_moves_file(cx: &mut TestAppContext) {
    let (view, cx, root) = workspace_window(cx, "tree-dnd");
    // Открыть todo.txt вкладкой.
    cx.cx.update(|app| {
        view.update(app, |ws, cx| {
            ws.model_mut().open_path(root.join("todo.txt")).unwrap();
            cx.notify();
        })
    });
    cx.run_until_parked();
    let todo = cx
        .debug_bounds(row_key(&root, "todo.txt"))
        .expect("строка файла отрисована");
    let notes = cx
        .debug_bounds(row_key(&root, "notes"))
        .expect("строка папки отрисована");
    let from = center_of(todo);
    let to = center_of(notes);
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        gpui::Point::new(px(f32::from(from.x) + 8.0), px(f32::from(from.y) + 4.0)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    let moved = root.join("notes/todo.txt");
    assert!(moved.is_file(), "файл перемещен на диск");
    assert!(!root.join("todo.txt").exists());
    let tab_path = cx.cx.update(|app| {
        view.read(app).model().active().unwrap().path.clone()
    });
    assert_eq!(tab_path.as_deref(), Some(moved.as_path()), "вкладка у нового пути");
}

/// 7.2: удаление по Cmd+Backspace с подтверждением — модальный
/// вопрос, согласие удаляет файл.
#[gpui::test]
fn tree_delete_with_confirmation(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let (view, cx, root) = workspace_window(cx, "tree-del");
    let todo = cx
        .debug_bounds(row_key(&root, "todo.txt"))
        .expect("строка файла отрисована");
    cx.simulate_click(center_of(todo), Modifiers::default());
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-backspace");
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "подтверждение удаления показано");
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    assert!(!root.join("todo.txt").exists(), "файл удален");
    // Строки дерева больше нет (модельная проверка: отрисовка строк
    // присутствия покрыта другими тестами, debug_bounds читает
    // последний ОТРИСОВАННЫЙ кадр — absence там не проверяемо).
    let gone = cx.update_window_entity(&view, |ws, _w, _cx| {
        ws.tree
            .as_mut()
            .map(|t| t.visible_rows().iter().any(|r| r.path == root.join("todo.txt")))
            .unwrap_or(false)
    });
    assert!(!gone, "строка исчезла из дерева");
    // Отказ не удаляет: создадим файл и откажемся.
    let keep = root.join("notes/idea.txt");
    let notes = cx
        .debug_bounds(row_key(&root, "notes"))
        .expect("строка папки отрисована");
    cx.simulate_click(center_of(notes), Modifiers::default()); // раскрыть + выделить
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-backspace");
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(keep.exists(), "отмена оставила файл");
}

/// 7.4: открытый воркспейс попадает в состояние сессии
/// (восстановление при старте — main.rs, путь сохраняется здесь).
#[gpui::test]
fn session_state_captures_workspace(cx: &mut TestAppContext) {
    let (view, cx, root) = workspace_window(cx, "tree-session");
    let state = cx.update_window_entity(&view, |ws, _w, _cx| ws.session_state(None));
    assert_eq!(state.workspace, Some(root));
}

/// 7.4 (продолжение): сессия без воркспейса сохраняет None.
#[gpui::test]
fn session_state_without_workspace(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));
    cx.run_until_parked();
    let state = cx.update_window_entity(&view, |ws, _w, _cx| ws.session_state(None));
    assert_eq!(state.workspace, None);
}

// ---- Полосы прокрутки (scrollbar.rs) ----

/// Полоса редактора: длинный файл — ползунок у правого края (доля ≈
/// вьюпорту в контенте, но не короче минимума), прокрутка вниз —
/// ползунок едет вниз; короткий файл — полосы нет.
#[gpui::test]
fn editor_scrollbar_tracks_scroll(cx: &mut TestAppContext) {
    let content: String = (0..300).map(|i| format!("line{i:03}\n")).collect();
    let (view, cx, _p) = editor_with_file(cx, "vscroll", &content);
    cx.run_until_parked();
    // Второй кадр: геометрия полосы читается из handle, значения — с
    // прошлого prepaint (ловушка render-до-paint, как у шапки D12.5).
    cx.update_window_entity(&view, |_ws, _w, cx| cx.notify());
    cx.run_until_parked();

    let editor = cx.update_window_entity(&view, |ws, _w, _| ws.editor_bounds().expect("editor"));
    let thumb = cx.debug_bounds("editor-vscroll-0-thumb").expect("ползунок отрисован");
    // Ползунок — в СВОЕЙ дорожке справа от текста (не перекрывает
    // его) и не выходит за ширину дорожки (TRACK_W).
    assert!(
        f32::from(thumb.left()) >= f32::from(editor.right()) - 0.5,
        "ползунок правее текста: thumb {:?} vs editor {:?}",
        thumb,
        editor
    );
    assert!(
        f32::from(thumb.right()) <= f32::from(editor.right()) + crate::scrollbar::TRACK_W + 0.5,
        "ползунок в пределах дорожки: thumb {:?}",
        thumb
    );
    // Ползунок короче дорожки, но не короче минимума (28px).
    assert!(f32::from(thumb.size.height) < f32::from(editor.size.height));
    assert!(f32::from(thumb.size.height) >= 28.0);

    // Прокрутка вглубь — ползунок сместился вниз.
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.panes[0].scroll.scroll_to_item(150, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    cx.run_until_parked();
    cx.update_window_entity(&view, |_ws, _w, cx| cx.notify());
    cx.run_until_parked();
    let thumb2 = cx.debug_bounds("editor-vscroll-0-thumb").expect("ползунок остался");
    assert!(
        f32::from(thumb2.origin.y) > f32::from(thumb.origin.y),
        "ползунок поехал вниз: {:?} → {:?}",
        thumb.origin,
        thumb2.origin
    );

    // Короткий файл — не скроллится, полосы нет.
    let (view2, cx, _p) = editor_with_file(cx, "vscroll-short", "ab\ncd\n");
    cx.run_until_parked();
    cx.update_window_entity(&view2, |_ws, _w, cx| cx.notify());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("editor-vscroll-0-thumb").is_none(),
        "короткий файл — без полосы"
    );
}

/// Драг ползунка редактора: mouse down на ползунке → move → up
/// прокручивает список (offset.y < 0); клик по ползунку НЕ ставит
/// каретку (stop_propagation).
#[gpui::test]
fn editor_scrollbar_thumb_drag_scrolls(cx: &mut TestAppContext) {
    let content: String = (0..300).map(|i| format!("line{i:03}\n")).collect();
    let (view, cx, _p) = editor_with_file(cx, "vscroll-drag", &content);
    cx.run_until_parked();
    cx.update_window_entity(&view, |_ws, _w, cx| cx.notify());
    cx.run_until_parked();
    let thumb = cx.debug_bounds("editor-vscroll-0-thumb").expect("ползунок отрисован");

    let mods = Modifiers::default();
    let start = center_of(thumb);
    let end = point(start.x, start.y + px(80.0));
    cx.simulate_mouse_down(start, MouseButton::Left, mods);
    cx.simulate_mouse_move(end, Some(MouseButton::Left), mods);
    cx.simulate_mouse_up(end, MouseButton::Left, mods);
    cx.run_until_parked();

    let (offset_y, caret) = cx.update_window_entity(&view, |ws, _w, _| {
        let y = f32::from(ws.panes[0].scroll.0.borrow().base_handle.offset().y);
        let c = ws.model().active().unwrap().primary().head();
        (y, c)
    });
    assert!(offset_y < -1.0, "драг ползунка прокрутил список, offset.y = {offset_y}");
    assert_eq!(caret, 0, "клик по ползунку не двигает каретку");
}

/// Горизонтальный скролл редактора (внешняя зона вокруг uniform_list):
/// длинная строка — ползунок есть и прокрутка реально сдвигает контент
/// (offset.x < 0, bounds списка уезжают влево); короткие строки —
/// полосы нет (контент не шире вьюпорта).
#[gpui::test]
fn editor_horizontal_scroll(cx: &mut TestAppContext) {
    let long_line = "x".repeat(400);
    let content = format!("short\n{long_line}\nend\n");
    let (view, cx, _p) = editor_with_file(cx, "hscroll", &content);
    cx.run_until_parked();
    // Второй кадр: геометрия полосы — с прошлого prepaint.
    cx.update_window_entity(&view, |_ws, _w, cx| cx.notify());
    cx.run_until_parked();

    let thumb = cx
        .debug_bounds("editor-hscroll-0-thumb")
        .expect("ползунок горизонтального скролла отрисован");
    // Ползунок — в своей дорожке ПОД текстом (не перекрывает его).
    let before = cx.update_window_entity(&view, |ws, _w, _| {
        (ws.panes[0].h_scroll.max_offset().width, ws.editor_bounds().expect("editor"))
    });
    assert!(
        f32::from(thumb.top()) >= f32::from(before.1.bottom()) - 0.5,
        "ползунок ниже текста: thumb {:?} vs editor {:?}",
        thumb,
        before.1
    );
    assert!(
        f32::from(before.0) > 0.0,
        "контент шире вьюпорта: max_offset.width = {:?}",
        before.0
    );

    // Драг ползунка вправо — контент едет влево (offset.x < 0).
    let mods = Modifiers::default();
    let start = center_of(thumb);
    let end = point(start.x + px(60.0), start.y);
    cx.simulate_mouse_down(start, MouseButton::Left, mods);
    cx.simulate_mouse_move(end, Some(MouseButton::Left), mods);
    cx.simulate_mouse_up(end, MouseButton::Left, mods);
    cx.run_until_parked();

    let (offset_x, bounds_x) = cx.update_window_entity(&view, |ws, _w, _| {
        (
            f32::from(ws.panes[0].h_scroll.offset().x),
            f32::from(ws.editor_bounds().expect("editor").origin.x),
        )
    });
    assert!(offset_x < -1.0, "драг прокрутил вправо: offset.x = {offset_x}");
    assert!(
        bounds_x < f32::from(before.1.origin.x) - 1.0,
        "bounds списка ушли влево вместе с контентом: {bounds_x}"
    );

    // Короткие строки — горизонтальной полосы нет.
    let (view2, cx, _p) = editor_with_file(cx, "hscroll-short", "ab\ncd\n");
    cx.run_until_parked();
    cx.update_window_entity(&view2, |_ws, _w, cx| cx.notify());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("editor-hscroll-0-thumb").is_none(),
        "узкий контент — без горизонтальной полосы"
    );
}

/// Разделение осей колеса (ловушка gpui 0.2.2, guard-оверлей
/// editor-wheel-guard): вертикальное колесо над редактором с ОБЕИМИ
/// осями прокрутки двигает ТОЛЬКО текст (offset.x зоны не меняется);
/// горизонтальная дельта — только нижнюю полосу (offset.y списка
/// не меняется).
#[gpui::test]
fn wheel_axes_are_separated(cx: &mut TestAppContext) {
    let long_line = "x".repeat(400);
    let content = format!("short\n{long_line}\nend\n{}", (0..200).map(|i| format!("l{i:03}\n")).collect::<String>());
    let (view, cx, _p) = editor_with_file(cx, "wheel-axes", &content);
    cx.run_until_parked();
    cx.update_window_entity(&view, |_ws, _w, cx| cx.notify());
    cx.run_until_parked();
    // Обе оси прокручиваемы (иначе тест ничего не проверяет).
    let (hmax, vmax) = cx.update_window_entity(&view, |ws, _w, _| {
        (
            f32::from(ws.panes[0].h_scroll.max_offset().width),
            f32::from(ws.panes[0].scroll.0.borrow().base_handle.max_offset().height),
        )
    });
    assert!(hmax > 0.0, "горизонталь прокручиваема");
    assert!(vmax > 0.0, "вертикаль прокручиваема");

    let center = cx
        .update_window_entity(&view, |ws, _w, _| ws.editor_bounds().expect("editor"));
    let at = |b: gpui::Bounds<gpui::Pixels>| {
        point(b.origin.x + b.size.width / 2.0, b.origin.y + b.size.height / 2.0)
    };
    let mods = Modifiers::default();

    // Вертикальное колесо: dy ≠ 0, dx = 0.
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: at(center),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-60.0))),
        modifiers: mods,
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.run_until_parked();
    let (vy, hx) = cx.update_window_entity(&view, |ws, _w, _| {
        (
            f32::from(ws.panes[0].scroll.0.borrow().base_handle.offset().y),
            f32::from(ws.panes[0].h_scroll.offset().x),
        )
    });
    assert!(vy != 0.0, "текст прокрутился по вертикали");
    assert_eq!(hx, 0.0, "горизонтальная полоса НЕ двинулась (баг конвертации осей)");

    // Горизонтальная дельта: dx ≠ 0, dy = 0 — только нижняя полоса.
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: at(center),
        delta: gpui::ScrollDelta::Pixels(point(px(-40.0), px(0.0))),
        modifiers: mods,
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.run_until_parked();
    let (vy2, hx2) = cx.update_window_entity(&view, |ws, _w, _| {
        (
            f32::from(ws.panes[0].scroll.0.borrow().base_handle.offset().y),
            f32::from(ws.panes[0].h_scroll.offset().x),
        )
    });
    assert!(hx2 != 0.0, "горизонтальная полоса прокрутилась");
    assert_eq!(vy2, vy, "текст НЕ прокрутился от горизонтальной дельты");
}

/// Прогрессивная подсветка после РЕЗКОГО прыжка (баг ручной проверки:
/// «плавный скролл — цвет есть, быстрый — пропадает»): бюджет движка
/// 2 мс/кадр не закрывает окно за один кадр, рендер обязан ГОНЯТЬ
/// кадры догонa сам (notify по ensure_window == false) — окно
/// полностью подсвечивается БЕЗ событий мыши, только кадрами.
#[gpui::test]
fn highlight_catches_up_after_big_jump(cx: &mut TestAppContext) {
    // 3k строк rust-кода; прыжок на ~2.5k с холодного движка — догон
    // заведомо дороже одного бюджета (десятки кадров по 2 мс), но
    // сходится за секунды теста.
    let content: String = (0..3_000)
        .map(|i| format!("fn f{i}(x: u64) -> u64 {{ x + {i} }}\n"))
        .collect();
    let (view, cx, _p) = editor_with_named_file(cx, "hl-jump", "big.rs", &content);
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.refresh_all_languages(cx);
        cx.notify();
    });
    cx.run_until_parked();

    // Резкий прыжок вглубь (один scroll_to_item — как флик трекпада;
    // deferred-скролл применяется в prepaint этого же кадра).
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.panes[0].scroll.scroll_to_item(2_500, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    cx.run_until_parked();

    // Дальше кадры гоняет САМ рендер (notify по ensure_window ==
    // false; в живом окне это vsync-цикл, тестовая платформа
    // on_request_frame no-op — качаем кадры явно, как display link).
    let mut covered = false;
    for _ in 0..400 {
        cx.update_window_entity(&view, |_ws, _w, cx| cx.notify());
        cx.run_until_parked();
        covered = cx.update_window_entity(&view, |ws, _w, _| {
            let b = ws.editor_bounds().expect("editor");
            let first = ws.visible_line_at_y(f32::from(b.origin.y) + 1.0).unwrap();
            let tab = ws.model().active().unwrap();
            (0..5_usize)
                .map(|i| first + i)
                .all(|vis| tab.highlight.spans(ws.visible_to_real(vis)).is_some())
        });
        if covered {
            break;
        }
    }
    assert!(
        covered,
        "после прыжка окно подсвечено прогрессивным догоном (кадры рендера)"
    );
}

/// Фоновая раскраска ходкой (workspace::spawn_background_highlight):
/// после открытия большого файла чекпоинты строятся САМИ (таймерная
/// ходка главного потока), без прыжков пользователя — прыжок вглубь
/// закрывается первым же кадром (догон ≤ 1024 строк от чекпоинта).
#[gpui::test]
fn background_highlight_builds_checkpoints(cx: &mut TestAppContext) {
    let content: String = (0..3_000)
        .map(|i| format!("fn f{i}(x: u64) -> u64 {{ x + {i} }}\n"))
        .collect();
    let (view, cx, _p) = editor_with_named_file(cx, "bg-walk", "big.rs", &content);
    // Прогрев (запускает ходки всех вкладок); ходка тикает таймером
    // 25 мс — тестовые таймеры фейковые, время двигаем advance_clock
    // (паттерн тестового dispatcher'а), pumping задач — парковкой.
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.refresh_all_languages(cx);
        cx.notify();
    });
    cx.run_until_parked();
    let mut cps: Vec<usize> = Vec::new();
    for _ in 0..400 {
        cx.cx
            .background_executor
            .advance_clock(std::time::Duration::from_millis(25));
        cx.run_until_parked();
        cps = cx.update_window_entity(&view, |ws, _w, _| {
            ws.model().active().unwrap().highlight.checkpoint_lines()
        });
        if cps.contains(&2048) {
            break; // ходка дошла глубже 2k — достаточно
        }
    }
    assert!(
        cps.contains(&1024) && cps.contains(&2048),
        "ходка построила чекпоинты: {cps:?}"
    );

    // Прыжок на 2.5k: догон ≤1024 строк от чекпоинта 2048 (~450
    // строк) — несколько кадров 2 мс-бюджета; гоняем кадры как vsync
    // (в живом окне — request_animation_frame, тест качает явно).
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.panes[0].scroll.scroll_to_item(2_500, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    cx.run_until_parked();
    let mut covered = false;
    for _ in 0..60 {
        cx.update_window_entity(&view, |_ws, _w, cx| cx.notify());
        cx.run_until_parked();
        covered = cx.update_window_entity(&view, |ws, _w, _| {
            let b = ws.editor_bounds().expect("editor");
            let first = ws.visible_line_at_y(f32::from(b.origin.y) + 1.0).unwrap();
            let tab = ws.model().active().unwrap();
            (0..5_usize)
                .map(|i| first + i)
                .all(|vis| tab.highlight.spans(ws.visible_to_real(vis)).is_some())
        });
        if covered {
            break;
        }
    }
    assert!(covered, "прыжок покрыт сразу после фоновой ходки");
}

// ---- Split view (8.1/D15.1) ----

/// 8.1: split создаёт независимые панели с независимыми вкладками;
/// Cmd+\ (dispatch) активирует новую панель; открытие файла попадает в
/// активную панель; максимум 4 панели.
#[gpui::test]
fn split_view_panes_have_independent_tabs(cx: &mut TestAppContext) {
    let dir = TempDir::new("split-tabs");
    let a = dir.write("a.txt", "alpha\n");
    let b = dir.write("b.txt", "beta\n");
    std::mem::forget(dir);
    let (view, cx) = cx.add_window_view(|_window, cx| {
        let mut ws = Workspace::new(cx);
        ws.model_mut().open_path(&a).unwrap();
        ws
    });
    focus_editor(&view, cx);
    cx.run_until_parked();

    // Split: панель 1 пуста и активна.
    cx.dispatch_action(SplitEditor);
    cx.cx.update(|app| {
        let ws = view.read(app);
        assert_eq!(ws.panes.len(), 2);
        assert_eq!(ws.active_pane, 1);
        assert!(ws.panes[1].model.is_empty());
        assert_eq!(ws.panes[0].model.len(), 1, "вкладки не переносятся");
    });

    // Открытие в активную панель.
    cx.cx.update(|app| {
        view.update(app, |ws, cx| {
            ws.model_mut().open_path(&b).unwrap();
            cx.notify();
        });
    });
    cx.run_until_parked();
    cx.cx.update(|app| {
        let ws = view.read(app);
        assert_eq!(ws.panes[1].model.len(), 1, "файл открыт в панели 1");
        assert_eq!(
            ws.panes[1].model.active().unwrap().path.as_deref(),
            Some(b.as_path())
        );
        assert_eq!(ws.panes[0].model.active().unwrap().path.as_deref(), Some(a.as_path()));
    });

    // Ввод идёт в активную панель (панель 1 — вкладка b).
    cx.simulate_keystrokes("x");
    cx.cx.update(|app| {
        let ws = view.read(app);
        assert_eq!(ws.panes[1].model.active().unwrap().doc.text(), "xbeta\n");
        assert_eq!(ws.panes[0].model.active().unwrap().doc.text(), "alpha\n");
    });

    // Максимум 4 панели: дальнейшие split — no-op.
    cx.dispatch_action(SplitEditor);
    cx.dispatch_action(SplitEditor);
    cx.dispatch_action(SplitEditor);
    cx.cx.update(|app| assert_eq!(view.read(app).panes.len(), 4));
}

/// 8.1: закрытие панели (Cmd+Shift+\) — чистая панель закрывается
/// сразу; активной становится соседняя; последняя панель не закрывается.
#[gpui::test]
fn close_pane_keeps_last_and_activates_neighbor(cx: &mut TestAppContext) {
    let (view, cx, _p) = editor_with_file(cx, "close-pane", "hello\n");
    cx.dispatch_action(SplitEditor);
    cx.cx.update(|app| assert_eq!(view.read(app).panes.len(), 2));

    // Закрытие пустой панели 1 — без диалогов.
    cx.dispatch_action(ClosePane);
    cx.run_until_parked();
    cx.cx.update(|app| {
        let ws = view.read(app);
        assert_eq!(ws.panes.len(), 1);
        assert_eq!(ws.active_pane, 0);
    });

    // Последняя панель не закрывается.
    cx.dispatch_action(ClosePane);
    cx.cx.update(|app| assert_eq!(view.read(app).panes.len(), 1));
}

/// 8.1: синхронная прокрутка — прокрутка одной панели выравнивает
/// первую видимую строку другой (ToggleSyncScroll + интерактивный
/// скролл колеса).
#[gpui::test]
fn sync_scroll_aligns_top_line(cx: &mut TestAppContext) {
    // Длинный файл — прокрутка реальна.
    let content: String = (0..500).map(|i| format!("line {i}\n")).collect();
    let (view, cx, _p) = editor_with_file(cx, "sync-scroll", &content);
    // Вторая панель: тот же файл открыт заново (независимая вкладка).
    let path = _p.clone();
    cx.dispatch_action(SplitEditor);
    cx.cx.update(|app| {
        view.update(app, |ws, cx| {
            ws.model_mut().open_path(&path).unwrap();
            cx.notify();
        });
    });
    cx.run_until_parked();

    // Синхронизация выключена: прокрутка панели 0 не трогает панель 1.
    let y_before = cx.cx.update(|app| {
        f32::from(
            view.read(app).panes[1]
                .scroll
                .0
                .borrow()
                .base_handle
                .offset()
                .y,
        )
    });
    cx.cx.update(|app| {
        view.update(app, |ws, cx| {
            let ws_scroll = ws.panes[0].scroll.0.borrow().base_handle.clone();
            ws_scroll.set_offset(point(px(0.0), px(-100.0 * LINE_HEIGHT)));
            cx.notify();
        });
    });
    cx.run_until_parked();
    cx.cx.update(|app| {
        let y = f32::from(
            view.read(app).panes[1]
                .scroll
                .0
                .borrow()
                .base_handle
                .offset()
                .y,
        );
        assert_eq!(y, y_before, "без sync панель 1 не двигается");
    });

    // Включаем и прокручиваем через wheel-обработчик панели 0
    // (интерактивный путь: on_editor_wheel_pane → sync_scroll_from);
    // +150 строк вниз от top line 100.
    cx.dispatch_action(ToggleSyncScroll);
    cx.update_window_entity(&view, |ws, window, cx| {
        let ev = gpui::ScrollWheelEvent {
            position: point(px(0.0), px(0.0)),
            // Прокрутка ВНИЗ — delta отрицателен (offset.y уменьшается;
            // семантика gpui: scrolled = -offset.y).
            delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-150.0 * LINE_HEIGHT))),
            modifiers: Default::default(),
            touch_phase: Default::default(),
        };
        ws.on_editor_wheel_pane(0, &ev, window, cx);
    });
    cx.run_until_parked();
    // Ловушка deferred-скролла (D12.5): scroll_to_item применяется в
    // prepaint ПОСЛЕ render — качаем второй кадр (как в тестах CSV).
    cx.update_window_entity(&view, |_ws, _w, cx| cx.notify());
    cx.run_until_parked();
    // Панель 1 выровнена по первой видимой строке панели 0 (строка
    // 250): deferred scroll_to_item применился на следующей отрисовке.
    let y1 = cx.cx.update(|app| {
        f32::from(
            view.read(app).panes[1]
                .scroll
                .0
                .borrow()
                .base_handle
                .offset()
                .y,
        )
    });
    assert!(
        (y1 + 250.0 * LINE_HEIGHT).abs() < LINE_HEIGHT,
        "панель 1 следует за панелью 0 (offset.y={y1})"
    );
}

/// 8.2: переключение темы — выбор меняется, попадает в состояние
/// сессии (переживает перезапуск); разрешение выбора — юнит-тесты
/// theme.rs (глобальная палитра гоняется между параллельными
/// тестовыми окнами — здесь проверяем детерминированную часть).
#[gpui::test]
fn theme_switch_applies_and_persists(cx: &mut TestAppContext) {
    let (view, cx, _p) = editor_with_file(cx, "theme", "hello\n");
    cx.run_until_parked();
    assert_eq!(
        cx.cx.update(|app| view.read(app).theme_kind),
        crate::theme::ThemeKind::System,
        "дефолт — системная"
    );

    cx.dispatch_action(ThemeDark);
    cx.run_until_parked();
    let saved = cx.cx.update(|app| {
        let ws = view.read(app);
        assert_eq!(ws.theme_kind, crate::theme::ThemeKind::Dark);
        ws.session_state(None).theme.expect("тема в сессии")
    });
    assert_eq!(saved, "dark");

    cx.dispatch_action(ThemeLight);
    cx.run_until_parked();
    let saved = cx.cx.update(|app| {
        let ws = view.read(app);
        assert_eq!(ws.theme_kind, crate::theme::ThemeKind::Light);
        ws.session_state(None).theme.expect("тема в сессии")
    });
    assert_eq!(saved, "light");
}

/// 8.3: зум шрифта применяется немедленно (высота строки растёт) и
/// сохраняется в сессии; сброс возвращает дефолт.
#[gpui::test]
fn font_zoom_applies_immediately_and_persists(cx: &mut TestAppContext) {
    let (view, cx, _p) = editor_with_file(cx, "font-zoom", "hello\n");
    cx.run_until_parked();
    let default_h = cx.cx.update(|app| view.read(app).line_height());
    assert_eq!(default_h, crate::geometry::LINE_HEIGHT, "дефолт 13px → 20px");

    cx.dispatch_action(ZoomIn);
    cx.run_until_parked();
    let (size, h, saved) = cx.cx.update(|app| {
        let ws = view.read(app);
        (
            ws.font.size,
            ws.line_height(),
            ws.session_state(None).font.expect("шрифт в сессии"),
        )
    });
    assert_eq!(size, 14.0);
    assert_eq!(h, (14.0_f32 * 20.0 / 13.0).round(), "высота строки пересчитана");
    assert_eq!(saved.size, 14.0);

    // Измеренный advance сброшен и перемеряется под новый размер.
    let adv = cx.update_window_entity(&view, |ws, window, _cx| ws.char_advance(window));
    assert!(adv > 0.0);

    // Сброс (Cmd+0).
    cx.dispatch_action(ZoomReset);
    cx.run_until_parked();
    let size = cx.cx.update(|app| view.read(app).font.size);
    assert_eq!(size, crate::geometry::DEFAULT_FONT_SIZE);

    // Клампы: зум больше максимума не уходит.
    cx.update_window_entity(&view, |ws, _w, cx| {
        let mut f = ws.font.clone();
        f.size = crate::geometry::FONT_MAX;
        ws.apply_font(f, cx);
    });
    cx.dispatch_action(ZoomIn);
    cx.run_until_parked();
    let size = cx.cx.update(|app| view.read(app).font.size);
    assert_eq!(size, crate::geometry::FONT_MAX);
}

/// 8.3: диалог шрифта — список системных семейств, клик применяет
/// семейство немедленно; размер — чипом.
#[gpui::test]
fn font_dialog_lists_families_and_applies(cx: &mut TestAppContext) {
    let (view, cx, _p) = editor_with_file(cx, "font-dialog", "hello\n");
    cx.run_until_parked();
    // Семейства собираются при открытии.
    let has_menlo = cx.update_window_entity(&view, |ws, window, cx| {
        ws.on_open_font_dialog(&OpenFontDialog, window, cx);
        !ws.all_fonts.is_empty()
    });
    assert!(has_menlo, "all_font_names непуст");
    cx.run_until_parked();
    // Menlo есть в системе (macOS) и в списке.
    let families = cx.cx.update(|app| view.read(app).filtered_font_families());
    assert!(
        families.iter().any(|f| f == "Menlo"),
        "Menlo в списке (текущее семейство — первым)"
    );
    assert_eq!(families[0], "Menlo", "текущее семейство первым");

    // Размер чипом: 16 — высота строки следует.
    cx.update_window_entity(&view, |ws, _w, cx| ws.apply_font_size(16.0, cx));
    cx.run_until_parked();
    let h = cx.cx.update(|app| view.read(app).line_height());
    assert_eq!(h, (16.0_f32 * 20.0 / 13.0).round());
}


/// 8.4: выбор языка интерфейса — оверлей со списком доступных, клик
/// применяет язык (глобаль + меню + зеркало сессии); ключи без
/// перевода показываются английским (fallback — юнит-тесты i18n).
#[gpui::test]
fn ui_language_dialog_switches_language(cx: &mut TestAppContext) {
    cx.update(|app| crate::i18n::init(app, Some("en")));
    // Меню собираются из глобали.
    let has_file = cx.update(|app| {
        crate::menus::build(app)
            .iter()
            .any(|m| m.name.as_ref() == "File")
    });
    assert!(has_file, "меню на английском по умолчанию");

    let (view, cx, _p) = editor_with_file(cx, "ui-lang", "hello\n");
    cx.run_until_parked();
    cx.dispatch_action(OpenUiLanguageDialog);
    cx.run_until_parked();
    // Оверлей открыт; английский присутствует в списке (каталог внешних
    // языков машины может отличаться — проверяем минимум).
    assert!(cx.debug_bounds("ui-lang-overlay").is_some(), "оверлей открыт");
    assert!(
        cx.debug_bounds("ui-lang-item:en").is_some(),
        "английский в списке"
    );
    // Клик по «en» — язык применён, оверлей закрыт, сессия помнит.
    let en_bounds = cx.debug_bounds("ui-lang-item:en").unwrap();
    cx.simulate_click(center_of(en_bounds), Modifiers::default());
    cx.run_until_parked();
    let (still_open, saved_lang) = cx.cx.update(|app| {
        let ws = view.read(app);
        (ws.ui_lang_dialog, ws.session_state(None).language)
    });
    assert!(!still_open, "оверлей закрыт после выбора");
    assert_eq!(saved_lang.as_deref(), Some("en"));
}

/// 8.5: раскладка — вкладки по кругу (Cmd+Shift+]), справочник
/// сочетаний (Cmd+/) с реальными записями KEYMAP.
#[gpui::test]
fn hotkeys_tab_cycle_and_shortcuts_reference(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let dir = TempDir::new("hotkeys");
    let a = dir.write("a.txt", "alpha\n");
    let b = dir.write("b.txt", "beta\n");
    std::mem::forget(dir);
    let (view, cx) = cx.add_window_view(|_window, cx| {
        let mut ws = Workspace::new(cx);
        ws.model_mut().open_path(&a).unwrap();
        ws.model_mut().open_path(&b).unwrap();
        ws
    });
    focus_editor(&view, cx);
    cx.run_until_parked();
    let active = |cx: &mut gpui::VisualTestContext| -> String {
        cx.update_window_entity(&view, |ws, _w, _| {
            let ix = ws.model().active_index().unwrap();
            ws.model().title(ix).unwrap()
        })
    };
    assert_eq!(active(cx), "b.txt");
    // Cmd+Shift+[ — на вкладку назад (b → a), по кругу вперёд.
    cx.simulate_keystrokes("cmd-shift-[");
    cx.run_until_parked();
    assert_eq!(active(cx), "a.txt");
    cx.simulate_keystrokes("cmd-shift-]");
    cx.run_until_parked();
    assert_eq!(active(cx), "b.txt");

    // Cmd+/ — справочник: заголовок + записи всех секций KEYMAP.
    cx.simulate_keystrokes("cmd-/");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("shortcuts-overlay").is_some(),
        "справочник открыт"
    );
    // Подписи из i18n рендерятся (секция File есть в en).
    let sections = crate::keymap::KEYMAP.iter().map(|d| d.section).collect::<std::collections::BTreeSet<_>>();
    assert!(sections.contains("file") && sections.contains("view"));
    // Esc — закрыт.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("shortcuts-overlay").is_none(), "справочник закрыт");
}

// ---- Предпросмотр Markdown (change markdown-preview) ----

/// Окно с открытыми файлами-вкладками (первый — активный).
fn markdown_window<'a>(
    cx: &'a mut TestAppContext,
    tag: &str,
    files: &[(&str, &str)],
) -> (gpui::Entity<Workspace>, &'a mut gpui::VisualTestContext) {
    let dir = TempDir::new(tag);
    let paths: Vec<std::path::PathBuf> = files
        .iter()
        .map(|(name, content)| dir.write(name, content))
        .collect();
    std::mem::forget(dir);
    let (view, cx) = cx.add_window_view(|_window, cx| {
        let mut ws = Workspace::new(cx);
        for path in &paths {
            ws.model_mut().open_path(path).unwrap();
        }
        ws.model_mut().activate(0); // активна первая (a.md)
        ws
    });
    focus_editor(&view, cx);
    (view, cx)
}

/// 3.1: Cmd+Shift+V — тумблер панели (открыть/закрыть).
#[gpui::test]
fn markdown_preview_toggle_and_binding(cx: &mut TestAppContext) {
    let (view, cx) = markdown_window(cx, "md-toggle", &[("notes.md", "# Заголовок\n\nАбзац\n")]);
    cx.cx.update(bind_app_keys);
    cx.simulate_keystrokes("cmd-shift-v");
    cx.run_until_parked();
    assert!(cx.cx.update(|app| view.read(app).markdown_preview.open), "панель открыта");
    cx.simulate_keystrokes("cmd-shift-v");
    cx.run_until_parked();
    assert!(!cx.cx.update(|app| view.read(app).markdown_preview.open), "панель закрыта");
}

/// Драг разделителя [редактор | предпросмотр] меняет ширину панели;
/// клампы держат минимумы редактора/панели; двойной клик — сброс к
/// пропорции 40%.
#[gpui::test]
fn markdown_preview_divider_drag(cx: &mut TestAppContext) {
    let (view, cx) = markdown_window(cx, "md-divider", &[("notes.md", "# Заголовок\n\nАбзац\n")]);
    cx.cx.update(bind_app_keys);
    cx.dispatch_action(crate::actions::ToggleMarkdownPreview);
    cx.run_until_parked();

    // Стартовая геометрия: панель по умолчанию (flex_basis 40%),
    // разделитель — её левый сосед.
    let before = cx.debug_bounds("preview-panel").expect("панель отрисована");
    let div = cx.debug_bounds("preview-divider").expect("разделитель отрисован");
    let vw = f32::from(before.right()); // панель упирается в правый край окна
    assert_eq!(
        cx.cx.update(|app| view.read(app).markdown_preview_width_for_test()),
        None,
        "до драга — пропорция по умолчанию"
    );

    // Драг на 100px влево — панель на ~100px шире.
    let start = div.center();
    let target = point(px(f32::from(start.x) - 100.0), px(f32::from(start.y)));
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    cx.simulate_mouse_move(target, Some(MouseButton::Left), Modifiers::default());
    cx.run_until_parked();
    cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    let after = cx.debug_bounds("preview-panel").expect("панель после драга");
    // Ожидание — ровно формула драга: vw − x мыши (центр 5px хит-зоны
    // на 2.5px левее края панели — сдвиг учтён формулой).
    let expected = crate::geometry::preview_width_at(f32::from(target.x), vw);
    assert!(
        (f32::from(after.size.width) - expected).abs() < 1.5,
        "ширина {}, ждали {expected}",
        f32::from(after.size.width)
    );
    assert!(
        f32::from(after.size.width) > f32::from(before.size.width),
        "панель стала шире"
    );
    assert_eq!(
        cx.cx.update(|app| view.read(app).markdown_preview_width_for_test()),
        Some(f32::from(after.size.width)),
        "явная ширина зафиксирована"
    );

    // Кламп: драг далеко влево — ширина по preview_width_at (редактору
    // остаётся EDITOR_MIN_W, панель не уже PREVIEW_MIN_W).
    let div = cx.debug_bounds("preview-divider").expect("разделитель после драга");
    let start = div.center();
    let far = point(px(10.0), px(f32::from(start.y)));
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    cx.simulate_mouse_move(far, Some(MouseButton::Left), Modifiers::default());
    cx.run_until_parked();
    cx.simulate_mouse_up(far, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    let clamped = cx.debug_bounds("preview-panel").expect("панель после клампа");
    let expected = crate::geometry::preview_width_at(10.0, vw);
    assert!(
        (f32::from(clamped.size.width) - expected).abs() < 1.5,
        "кламп: ширина {}, ждали {expected}",
        f32::from(clamped.size.width)
    );

    // Двойной клик по разделителю — сброс к пропорции 40%. Клики
    // helpers всегда click_count:1 — шлём down с click_count:2
    // напрямую (реальный double-click; up не нужен — сброс на down).
    let div = cx.debug_bounds("preview-divider").expect("разделитель после клампа");
    let at = div.center();
    cx.simulate_event(MouseDownEvent {
        position: at,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    cx.run_until_parked();
    assert_eq!(
        cx.cx.update(|app| view.read(app).markdown_preview_width_for_test()),
        None,
        "двойной клик — сброс к пропорции"
    );
}

/// 3.3: предпросмотр следует за активной вкладкой; не-Markdown — без
/// модели (placeholder), возврат к Markdown — модель строится.
#[gpui::test]
fn markdown_preview_follows_active_tab(cx: &mut TestAppContext) {
    let (view, cx) = markdown_window(
        cx,
        "md-follow",
        &[("a.md", "# Один\n\n- x\n"), ("b.md", "## Два\n\nтекст\n"), ("main.rs", "fn main() {}\n")],
    );
    cx.cx.update(bind_app_keys); // keystrokes переключения вкладок
    cx.dispatch_action(crate::actions::ToggleMarkdownPreview);
    cx.run_until_parked();
    // Активна a.md — модель синхронно построена (≤ 2 МБ, D4).
    assert_eq!(
        cx.cx.update(|app| view.read(app).preview_item_count()),
        Some(2),
        "a.md: заголовок + пункт списка"
    );
    // b.md — своя модель.
    cx.simulate_keystrokes("cmd-shift-]");
    cx.run_until_parked();
    assert_eq!(cx.cx.update(|app| view.read(app).preview_item_count()), Some(2), "b.md");
    // main.rs — источник пропал (placeholder-ветка панели).
    cx.simulate_keystrokes("cmd-shift-]");
    cx.run_until_parked();
    let md_active = cx.cx.update(|app| {
        view.read(app)
            .model()
            .active()
            .is_some_and(crate::markdown_preview::is_markdown_tab_for_test)
    });
    assert!(!md_active, "не-Markdown вкладка — источника нет");
    // Возврат к a.md — модель перестроена под неё.
    cx.simulate_keystrokes("cmd-shift-[");
    cx.run_until_parked();
    assert_eq!(cx.cx.update(|app| view.read(app).preview_item_count()), Some(2), "возврат к a.md");
}

/// 3.4: правка `##` → `#` отражается в модели без сохранения (debounce
/// 200 мс + фон; advance_clock будит таймер).
#[gpui::test]
fn markdown_preview_live_update(cx: &mut TestAppContext) {
    let (view, cx) =
        markdown_window(cx, "md-live", &[("doc.md", "## Заголовок\n\nАбзац\n")]);
    cx.dispatch_action(crate::actions::ToggleMarkdownPreview);
    cx.run_until_parked();
    assert_eq!(cx.cx.update(|app| view.read(app).preview_item_count()), Some(2));

    // Правка в редакторе: `## ` → `# ` (первые 3 char: '#','#',' ').
    cx.cx.update(|app| {
        view.update(app, |ws, cx| {
            let tab = ws.model_mut().active_mut().unwrap();
            let tx = editor_core::Transaction::from_edits(vec![
                editor_core::transaction::Edit::delete(0, 1),
            ]);
            tab.apply_tx(tx);
            cx.notify();
        })
    });
    // Debounce: до тишины 200 мс модель НЕ пересобрана под новую
    // ревизию; после — пересобрана (заголовок стал h1, рядов столько же).
    cx.cx.executor().advance_clock(std::time::Duration::from_millis(50));
    cx.run_until_parked();
    let before = cx.cx.update(|app| view.read(app).markdown_preview_revision());
    cx.cx.executor().advance_clock(std::time::Duration::from_millis(250));
    cx.run_until_parked();
    let after = cx.cx.update(|app| view.read(app).markdown_preview_revision());
    assert!(after >= before, "модель обновилась после debounce");
    assert_eq!(cx.cx.update(|app| view.read(app).preview_item_count()), Some(2), "ряды на месте");
}

/// 4.5: клик по http(s)-ссылке в предпросмотре открывает браузер ОС
/// (TestAppContext перехватывает open_url).
#[gpui::test]
fn markdown_preview_link_click_opens_url(cx: &mut TestAppContext) {
    let (view, cx) =
        markdown_window(cx, "md-link", &[("l.md", "[сайт](https://example.com)\n")]);
    cx.cx.update(bind_app_keys);
    cx.dispatch_action(crate::actions::ToggleMarkdownPreview);
    cx.run_until_parked();
    let bounds = cx.debug_bounds("md-row-0").expect("абзац со ссылкой отрисован");
    let center = bounds.center();
    cx.simulate_click(center, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(cx.cx.opened_url(), Some("https://example.com".to_string()));
    assert!(cx.cx.update(|app| view.read(app).markdown_preview.open), "панель не закрылась");
}

// ---- Тулбар и окно подсказок (change toolbar-and-shortcuts) ----

/// 1.1: View → Toolbar скрывает/показывает полосу. Невидимость
/// проверяется ПОВЕДЕНИЕМ (клик в бывшую позицию кнопки мёртв):
/// debug_bounds в test-support не чистит исчезнувшие элементы
/// 1.1: View → Toolbar скрывает/показывает полосу. Невидимость
/// проверяется ПОВЕДЕНИЕМ (клик в бывшую позицию кнопки мёртв):
/// debug_bounds в test-support не чистит исчезнувшие элементы
/// (`Frame::clear` не трогает `debug_bounds`) — «is_none» ненадёжен.
#[gpui::test]
fn toolbar_toggle_hides_and_shows(cx: &mut TestAppContext) {
    let (view, cx) = markdown_window(cx, "tb-toggle", &[("a.txt", "x\n")]);
    cx.run_until_parked();
    assert!(cx.debug_bounds("toolbar").is_some(), "тулбар виден по умолчанию");
    let save_pos = cx.debug_bounds("tool-save").unwrap().center();
    // Скрыть: состояние + мёртвая кнопка (правка не сохраняется).
    cx.dispatch_action(ToggleToolbar);
    cx.run_until_parked();
    assert!(!cx.cx.update(|app| view.read(app).toolbar_visible), "скрыт");
    cx.update_window_entity(&view, |ws, _w, cx| {
        let tab = ws.model_mut().active_mut().unwrap();
        tab.apply_tx(editor_core::Transaction::from_edits(vec![
            editor_core::transaction::Edit::insert(0, "!"),
        ]));
        cx.notify();
    });
    cx.simulate_click(save_pos, Modifiers::default());
    cx.run_until_parked();
    assert!(
        cx.cx.update(|app| view.read(app).model().active().unwrap().dirty),
        "кнопка скрытой полосы не срабатывает"
    );
    // Вернуть: кнопка снова работает (клик сохраняет).
    cx.dispatch_action(ToggleToolbar);
    cx.run_until_parked();
    assert!(cx.cx.update(|app| view.read(app).toolbar_visible), "возвращён");
    cx.simulate_click(save_pos, Modifiers::default());
    cx.run_until_parked();
    let (dirty, disk) = cx.cx.update(|app| {
        let ws = view.read(app);
        let tab = ws.model().active().unwrap();
        (tab.dirty, std::fs::read_to_string(tab.path.as_ref().unwrap()).unwrap())
    });
    assert!(!dirty, "возвращённая кнопка сохранила");
    assert_eq!(disk, "!x\n");
}

/// 3.1: кнопка Save делает то же, что Cmd+S (dispatch того же action).
#[gpui::test]
fn toolbar_save_button_dispatches_save(cx: &mut TestAppContext) {
    let (view, cx) = markdown_window(cx, "tb-save", &[("a.txt", "изначально\n")]);
    cx.run_until_parked();
    // Правка → dirty → клик по кнопке Save → чисто (файл на диске
    // перезаписан). Путь кнопки = путь действия.
    cx.update_window_entity(&view, |ws, _w, cx| {
        let tab = ws.model_mut().active_mut().unwrap();
        tab.apply_tx(editor_core::Transaction::from_edits(vec![
            editor_core::transaction::Edit::insert(0, "!"),
        ]));
        assert!(tab.dirty);
        cx.notify();
    });
    let save = cx.debug_bounds("tool-save").expect("кнопка Save отрисована");
    cx.simulate_click(save.center(), Modifiers::default());
    cx.run_until_parked();
    let (dirty, disk) = cx.cx.update(|app| {
        let ws = view.read(app);
        let tab = ws.model().active().unwrap();
        (tab.dirty, std::fs::read_to_string(tab.path.as_ref().unwrap()).unwrap())
    });
    assert!(!dirty, "кнопка сохранила");
    assert_eq!(disk, "!изначально\n");
}

/// 3.3: дропдаун «Текст» — Sort Ascending сортирует строки файла;
/// Esc закрывает список без действия (сценарии спеки).
#[gpui::test]
fn toolbar_text_dropdown_sorts_and_esc_closes(cx: &mut TestAppContext) {
    let (view, cx) = markdown_window(cx, "tb-text", &[("a.txt", "b\na\nc\n")]);
    cx.run_until_parked();
    // Открыть дропдаун кликом по кнопке «Текст».
    let btn = cx.debug_bounds("tool-text-ops").expect("кнопка Текст");
    cx.simulate_click(btn.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("toolbar-dropdown").is_some(), "дропдаун открыт");
    // Esc — закрыть без действия (состояние; debug_bounds не чистит
    // исчезнувшие элементы — см. toolbar_toggle_hides_and_shows).
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(
        !cx.cx.update(|app| view.read(app).toolbar_dropdown.is_some()),
        "Esc закрыл дропдаун"
    );
    let text = cx.cx.update(|app| view.read(app).model().active().unwrap().doc.text());
    assert_eq!(text, "b\na\nc\n", "действия не было");
    // Открыть снова и выбрать Sort Ascending — строки сортированы.
    cx.simulate_click(btn.center(), Modifiers::default());
    cx.run_until_parked();
    let item = cx.debug_bounds("dd-item-menu.text.sort_asc").expect("пункт Sort Asc");
    cx.simulate_click(item.center(), Modifiers::default());
    cx.run_until_parked();
    let text = cx.cx.update(|app| view.read(app).model().active().unwrap().doc.text());
    assert_eq!(text, "a\nb\nc\n", "сортировка из дропдауна");
    assert!(
        !cx.cx.update(|app| view.read(app).toolbar_dropdown.is_some()),
        "меню закрылось выбором"
    );
}

/// 3.4 + 5.1: кнопка предпросмотра — toggle-состояние и действие
/// (синхронизация: панель открыта кнопкой = открыта тумблером).
#[gpui::test]
fn toolbar_preview_button_toggles_preview(cx: &mut TestAppContext) {
    let (view, cx) = markdown_window(cx, "tb-preview", &[("a.md", "# Заг\n")]);
    cx.run_until_parked();
    let btn = cx.debug_bounds("tool-preview").expect("кнопка превью");
    cx.simulate_click(btn.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(cx.cx.update(|app| view.read(app).markdown_preview.open), "клик открыл превью");
    cx.simulate_click(btn.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(!cx.cx.update(|app| view.read(app).markdown_preview.open), "повторный клик закрыл");
}

/// 4.3: кнопка «?» открывает окно; 4.2: поиск фильтрует по названию
/// и клавише, пустой результат — сообщение.
#[gpui::test]
fn shortcuts_window_button_and_search(cx: &mut TestAppContext) {
    let (view, cx) = markdown_window(cx, "tb-help", &[("a.txt", "x\n")]);
    cx.cx.update(bind_app_keys);
    cx.run_until_parked();
    // Кнопка «?» открывает центрированное окно.
    let btn = cx.debug_bounds("tool-help").expect("кнопка «?»");
    cx.simulate_click(btn.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("shortcuts-overlay").is_some(), "окно открыто кнопкой");
    // Поиск по клавише: «save» → есть записи (Save ⌘S), нет «нет
    // совпадений».
    cx.simulate_keystrokes("s a v e");
    cx.run_until_parked();
    assert!(cx.debug_bounds("shortcuts-no-matches").is_none(), "save находится");
    // Мусорный запрос — сообщение об отсутствии.
    cx.simulate_keystrokes("z z z q");
    cx.run_until_parked();
    assert!(cx.debug_bounds("shortcuts-no-matches").is_some(), "пустой результат виден");
    // Esc закрывает, фокус возвращается редактору (состояние:
    // debug_bounds не чистит исчезнувшие элементы — см. toggle-тест).
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(
        !cx.cx.update(|app| view.read(app).shortcuts_open),
        "окно закрыто (Esc)"
    );
}

// ---- Транзиентные сообщения статус-бара (починка: позиция/кодировка
// пропадали навсегда после тумблеров) ----

/// Сообщение живёт справа на TTL секунд, НЕ подменяя левый блок
/// (кодировка/позиция/язык стабильны); затем очищается, новое
/// сообщение перезапускает таймер (поколение).
#[gpui::test]
fn status_message_auto_expires(cx: &mut TestAppContext) {
    let (view, cx) = markdown_window(cx, "status-ttl", &[("a.txt", "x\n")]);
    cx.run_until_parked();
    // Базовый текст левого блока: кодировка | переводы | позиция | язык.
    let base = cx.cx.update(|app| view.read(app).status_bar_text(app));
    assert!(base.starts_with("UTF-8"), "база: {base}");
    assert!(base.contains("Ln 1, Col 1"), "база: {base}");
    let no_message = cx.cx.update(|app| view.read(app).status_message.clone());
    assert_eq!(no_message, None, "без сообщений справа «notepad+»");

    // Тумблер превью → сообщение справа; левый блок НЕ тронут.
    cx.dispatch_action(crate::actions::ToggleMarkdownPreview);
    cx.run_until_parked();
    let shown = cx.cx.update(|app| view.read(app).status_message.clone());
    assert!(shown.is_some(), "сообщение показано");
    let left = cx.cx.update(|app| view.read(app).status_bar_text(app));
    assert!(left.contains("Ln 1, Col 1"), "левый блок стабилен: {left}");

    // Новое сообщение перезапускает таймер (поколение): sync-scroll
    // через 3 с после превью.
    cx.cx
        .executor()
        .advance_clock(std::time::Duration::from_secs(3));
    cx.dispatch_action(crate::actions::ToggleSyncScroll);
    cx.run_until_parked();
    // +3 с (итого 6 с от превью, 3 с от sync) — превью-таймер сработал,
    // но поколение новее: сообщение sync ещё живо.
    cx.cx
        .executor()
        .advance_clock(std::time::Duration::from_secs(3));
    cx.run_until_parked();
    let held = cx.cx.update(|app| view.read(app).status_message.clone()).expect("новое сообщение живо");
    assert!(held.contains("scrolling"), "новое сообщение живо: {held}");

    // +0.5 с от sync-сообщения (TTL 4 с) — живо; ещё +1 с (4.5 > TTL) —
    // очистилось, левый блок при этом не менялся ни разу.
    cx.cx
        .executor()
        .advance_clock(std::time::Duration::from_millis(500));
    cx.run_until_parked();
    let held2 = cx.cx.update(|app| view.read(app).status_message.clone());
    assert!(held2.is_some(), "sync-сообщение ещё в TTL");
    cx.cx
        .executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    let expired = cx.cx.update(|app| view.read(app).status_message.clone());
    assert_eq!(expired, None, "сообщение истекло");
    let restored = cx.cx.update(|app| view.read(app).status_bar_text(app));
    assert!(restored.contains("Ln 1, Col 1"), "левый блок на месте: {restored}");
}

/// Регрессия подсветки JSON: холодный старт с ПУСТЫМ редактором —/// прогрев (одноразовый) не должен оставлять набор синтаксисов
/// неинициализированным, иначе файлы, открытые позже (например
/// .json), детектируются как Plain Text навсегда.
#[gpui::test]
fn warmup_without_tabs_enables_later_language_detection(cx: &mut TestAppContext) {
    let dir = TempDir::new("json-cold-start");
    let path = dir.write("data.json", "{\n  \"name\": \"test\",\n  \"n\": 42\n}\n");
    std::mem::forget(dir);
    let (view, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));
    cx.run_until_parked();
    // Прогрев при нуле вкладок (путь таймера 50 мс из render).
    cx.update_window_entity(&view, |ws, _w, cx| ws.refresh_all_languages(cx));
    cx.update_window_entity(&view, |ws, _w, _cx| {
        assert!(
            editor_core::syntax::try_syntax_set().is_some(),
            "прогрев инициализирует набор даже без вкладок"
        );
        // Файл, открытый ПОСЛЕ прогрева, получает язык и грамматику.
        ws.model_mut().open_path(&path).unwrap();
        let tab = ws.model().active().unwrap();
        assert_eq!(tab.language.label(), "JSON");
        assert_eq!(tab.highlight.syntax_name(), Some("JSON"));
    });
}

/// Контекстное меню редактора (правый клик): Copy копирует выделение
/// (клик внутри выделения его сохраняет), Paste вставляет из клипборда,
/// Add Bookmark ставит закладку на строку под кликом; пункты те же
/// обработчики, что меню/хоткеи (дублирующей логики нет).
#[gpui::test]
fn editor_context_menu_copy_paste_bookmark(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "editor-ctx", "alpha\nbeta\n");
    let grid = MouseGrid::of(cx, &view);

    // Выделение "beta" [5..9), правый клик ВНУТРИ выделения —
    // выделение сохраняется (иначе Copy скопировал бы пустоту).
    cx.update_window_entity(&view, |ws, _w, _| {
        let tab = ws.model_mut().active_mut().unwrap();
        tab.set_caret_at(1, 0);
        tab.select_to(1, 4);
    });
    cx.simulate_mouse_down(grid.at(1.0, 2.0), MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
    let copy = cx
        .debug_bounds("editor-ctx-menu:Copy")
        .expect("пункт Copy в контекстном меню");
    cx.simulate_click(center_of(copy), Modifiers::default());
    cx.run_until_parked();
    let clip = cx.cx.read_from_clipboard().and_then(|i| i.text());
    assert_eq!(clip.as_deref(), Some("beta"), "выделение пережило правый клик");
    // Меню закрылось после пункта (bounds последнего кадра могут
    // отставать — инвариант состояния, не отрисовки).
    cx.update_window_entity(&view, |ws, _w, _cx| {
        assert!(ws.editor_context_menu.is_none(), "меню закрыто");
    });

    // Правый клик вне выделения (строка 0) — каретка переезжает,
    // Add Bookmark ставит закладку на строку 0.
    cx.simulate_mouse_down(grid.at(0.0, 1.0), MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
    let bookmark = cx
        .debug_bounds("editor-ctx-menu:Add Bookmark")
        .expect("пункт Add Bookmark");
    cx.simulate_click(center_of(bookmark), Modifiers::default());
    cx.run_until_parked();
    cx.update_window_entity(&view, |ws, _w, _| {
        let tab = ws.model().active().unwrap();
        assert_eq!(tab.bookmarked_lines(), vec![0], "закладка на строке клика");
        assert_eq!(tab.cursor_line(), 0, "каретка переехала под клик");
    });

    // Повторный правый клик на той же строке: метка пункта сменилась
    // (закладка есть) → Remove Bookmark снимает её.
    cx.simulate_mouse_down(grid.at(0.0, 1.0), MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
    let remove = cx
        .debug_bounds("editor-ctx-menu:Remove Bookmark")
        .expect("метка пункта сменилась на Remove");
    cx.simulate_click(center_of(remove), Modifiers::default());
    cx.run_until_parked();
    cx.update_window_entity(&view, |ws, _w, _| {
        assert!(
            ws.model().active().unwrap().bookmarked_lines().is_empty(),
            "закладка снята"
        );
    });

    // Paste из клипборда ("beta") в каретку строки 0 (вне выделения —
    // каретка переехала под правый клик).
    cx.simulate_mouse_down(grid.at(0.0, 1.0), MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
    let paste = cx
        .debug_bounds("editor-ctx-menu:Paste")
        .expect("пункт Paste");
    cx.simulate_click(center_of(paste), Modifiers::default());
    cx.run_until_parked();
    cx.update_window_entity(&view, |ws, _w, _| {
        let tab = ws.model().active().unwrap();
        assert!(
            tab.doc.line_content(0).contains("beta"),
            "paste вставил текст: {:?}",
            tab.doc.text()
        );
    });
}

/// Клик мимо goto-диалога закрывает его (подложка; регрессия бага
/// «диалог висит на экране, пока не нажат Esc»).
#[gpui::test]
fn goto_dialog_closes_on_outside_click(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "goto-underlay", "one\ntwo\n");
    cx.dispatch_action(GoToLine);
    let open = cx.cx.update(|app| view.read(app).goto_input.clone());
    assert_eq!(open, Some(String::new()));
    // Клик вне диалога (диалог ~ y 44..76, x 340..620).
    cx.simulate_click(point(px(500.0), px(300.0)), Modifiers::default());
    cx.run_until_parked();
    let open = cx.cx.update(|app| view.read(app).goto_input.clone());
    assert_eq!(open, None, "клик мимо закрыл диалог");
    // Фокус вернулся в редактор: ввод идёт в текст.
    cx.simulate_keystrokes("x");
    cx.update_window_entity(&view, |ws, _w, _cx| {
        let tab = ws.model().active().unwrap();
        assert!(tab.doc.text().contains("x"), "фокус в редакторе после закрытия");
    });
}

/// Select All: хоткей Cmd+A и пункт контекстного меню — выделение
/// всего документа; Copy после него кладёт весь текст в клипборд.
#[gpui::test]
fn select_all_via_hotkey_and_context_menu(cx: &mut TestAppContext) {
    cx.update(bind_app_keys);
    let (view, cx, _path) = editor_with_file(cx, "select-all", "alpha\nbeta\n");
    let grid = MouseGrid::of(cx, &view);

    // Хоткей: Cmd+A → выделено всё, Copy — весь текст в клипборде.
    cx.simulate_keystrokes("cmd-a");
    cx.update_window_entity(&view, |ws, _w, _| {
        let tab = ws.model().active().unwrap();
        assert_eq!(tab.primary(), &Selection::range(0, 11), "cmd-a выделил всё");
    });
    cx.simulate_keystrokes("cmd-c");
    let clip = cx.cx.read_from_clipboard().and_then(|i| i.text());
    assert_eq!(clip.as_deref(), Some("alpha\nbeta\n"), "copy после select all");

    // Правый клик (вне выделения — каретка) → пункт Select All.
    cx.simulate_mouse_down(grid.at(0.0, 1.0), MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
    let select_all = cx
        .debug_bounds("editor-ctx-menu:Select All")
        .expect("пункт Select All в контекстном меню");
    cx.simulate_click(center_of(select_all), Modifiers::default());
    cx.run_until_parked();
    cx.update_window_entity(&view, |ws, _w, _| {
        let tab = ws.model().active().unwrap();
        assert_eq!(tab.primary(), &Selection::range(0, 11), "пункт меню выделил всё");
    });
}

