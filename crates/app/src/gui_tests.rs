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
        ws.model.open_path(&path).unwrap();
        ws
    });
    cx.focus(&view);
    (view, cx, path)
}

/// Ввод с клавиатуры доходит до редактора и идёт через editor-core
/// (документ + dirty + позиция каретки).
#[gpui::test]
fn typing_edits_document_and_marks_dirty(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "typing", "");
    cx.simulate_keystrokes("h e l l o");
    let (text, dirty, head) = cx.cx.update(|app| {
        let tab = view.read(app).model.active().unwrap();
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
        let tab = view.read(app).model.active().unwrap();
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
        let tab = view.read(app).model.active().unwrap();
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
        .update(|app| view.read(app).model.active().unwrap().dirty);
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
        .update(|app| view.read(app).model.active().unwrap().doc.text());
    assert_eq!(after_undo, "");
    cx.dispatch_action(Redo);
    let after_redo = cx
        .cx
        .update(|app| view.read(app).model.active().unwrap().doc.text());
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
    let line = cx.cx.update(|app| view.read(app).model.active().unwrap().cursor_line());
    assert_eq!(line, page);
    cx.simulate_keystrokes("pageup");
    let line = cx.cx.update(|app| view.read(app).model.active().unwrap().cursor_line());
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
        let tab = view.read(app).model.active().unwrap();
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
    let sel = cx.cx.update(|app| view.read(app).model.active().unwrap().cursors.last().copied());
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
    let sel = cx.cx.update(|app| view.read(app).model.active().unwrap().cursors.last().copied());
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
        let tab = view.read(app).model.active().unwrap();
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
        let tab = view.read(app).model.active().unwrap();
        (tab.cursor_line(), view.read(app).goto_input.clone())
    });
    assert_eq!(line, 0, "escape leaves caret in place");
    assert_eq!(open, None);

    // Clamp: строка 999 → последняя строка (3).
    cx.dispatch_action(GoToLine);
    cx.simulate_keystrokes("9 9 9 enter");
    let (line, col) = cx.cx.update(|app| {
        let tab = view.read(app).model.active().unwrap();
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
    let count = cx.cx.update(|app| view.read(app).model.active().unwrap().cursors.len());
    assert_eq!(count, 2, "cmd+click adds a cursor");
    cx.simulate_keystrokes("z");
    let (text, cursors) = cx.cx.update(|app| {
        let tab = view.read(app).model.active().unwrap();
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
    let cursors = cx.cx.update(|app| view.read(app).model.active().unwrap().cursors.clone());
    // Строки 0..=2, выделение [1..3).
    assert_eq!(cursors.len(), 3);
    assert_eq!(cursors[0], Selection::range(1, 3));
    assert_eq!(cursors[2], Selection::range(13, 15));
    cx.simulate_keystrokes("X");
    let text = cx.cx.update(|app| view.read(app).model.active().unwrap().doc.text());
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
        let tab = view.read(app).model.active().unwrap();
        (tab.cursors.len(), *tab.primary())
    });
    assert_eq!(count, 2);
    assert_eq!(primary, Selection::range(8, 11));
    cx.simulate_keystrokes("Q");
    let text = cx.cx.update(|app| view.read(app).model.active().unwrap().doc.text());
    // Ввод заменяет ВСЕ выделения (оба вхождения "foo").
    assert_eq!(text, "Q bar Q\nbaz foo\n");
}

/// Биндинги группы 3+4 как в main.rs (в тестах main() не запускается,
/// keymap приложения пуст — регистрируем явно для E2E-проверки клавиш).
fn bind_app_keys(cx: &mut gpui::App) {
    use gpui::KeyBinding;
    cx.bind_keys([
        KeyBinding::new("cmd-z", Undo, None),
        KeyBinding::new("cmd-g", GoToLine, None),
        KeyBinding::new("cmd-d", AddNextMatch, None),
        KeyBinding::new("cmd-f2", ToggleBookmark, None),
        KeyBinding::new("f2", NextBookmark, None),
    ]);
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
    let head = cx.cx.update(|app| view.read(app).model.active().unwrap().primary().head());
    assert_eq!(head, 5, "каретка внутри слова");
    cx.simulate_keystrokes("cmd-d");
    let sel = cx.cx.update(|app| *view.read(app).model.active().unwrap().primary());
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
    let sel = cx.cx.update(|app| *view.read(app).model.active().unwrap().primary());
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
    let sel = cx.cx.update(|app| *view.read(app).model.active().unwrap().primary());
    assert_eq!(sel, Selection::range(12, 15), "тройной клик — строка с терминатором");
}

/// 4.3: статус-бар показывает позицию ГЛАВНОГО (последнего) курсора
/// и число курсоров.
#[gpui::test]
fn status_bar_uses_primary_cursor(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "status-multi", "one\ntwo\n");
    cx.update_window_entity(&view, |ws, _window, _| {
        let tab = ws.model.active_mut().unwrap();
        tab.set_cursors(vec![Selection::caret(0), Selection::caret(4)]);
    });
    let text = cx.cx.update(|app| view.read(app).status_bar_text());
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
        let tab = view.read(app).model.active().unwrap();
        (*tab.primary(), tab.cursor_line(), tab.cursor_col())
    });
    assert_eq!((line, col), (30, 38), "голова на строке 31, колонка 38");
    // Модель: [начало строки 20 .. строка 30 + 38]; строка = 67
    // символов + \n = шаг 68.
    let ls20 = 20 * 68;
    assert_eq!(sel, Selection::range(ls20, 30 * 68 + 38));
    // Рендер-данные строки 30 (последней): ЧАСТИЧНО [0..38].
    let (carets, ranges) = line_carets(&[sel], &editor_core::Document::from_str(&content), 30, 67);
    assert_eq!(ranges, vec![(0, 38)], "последняя строка — частичная подсветка");
    // Рендер-данные строки 20 (первой): [0..67] — целиком (якорь в кол 0).
    let (_, ranges20) = line_carets(&[sel], &editor_core::Document::from_str(&content), 20, 67);
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
        ws.model
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
        ws.model.active_mut().unwrap().toggle_bookmark_at(1);
        ws.model.active_mut().unwrap().toggle_bookmark_at(3);
    });
    cx.dispatch_action(NextBookmark); // со строки 0 → 1
    let line = cx.cx.update(|app| view.read(app).model.active().unwrap().cursor_line());
    assert_eq!(line, 1);
    cx.dispatch_action(NextBookmark); // → 3
    let line = cx.cx.update(|app| view.read(app).model.active().unwrap().cursor_line());
    assert_eq!(line, 3);
    cx.dispatch_action(PrevBookmark); // → 1
    let line = cx.cx.update(|app| view.read(app).model.active().unwrap().cursor_line());
    assert_eq!(line, 1);
    // Toggle на строке главной каретки (1) снимает закладку.
    cx.dispatch_action(ToggleBookmark);
    let bms = cx
        .cx
        .update(|app| view.read(app).model.active().unwrap().bookmarked_lines());
    assert_eq!(bms, vec![3]);
}

/// 4.4: закладки переживают undo/redo (GUI-путь: ввод через
/// keystrokes, undo действием).
#[gpui::test]
fn bookmarks_survive_undo_gui(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "bm-undo", "one\ntwo\n");
    cx.update_window_entity(&view, |ws, _w, _| {
        ws.model.active_mut().unwrap().toggle_bookmark_at(1);
    });
    cx.simulate_keystrokes("up"); // каретка на строку 0
    cx.simulate_keystrokes("N enter"); // новая строка сверху
    let text = cx.cx.update(|app| view.read(app).model.active().unwrap().doc.text());
    assert_eq!(text, "N\none\ntwo\n");
    let bm = cx
        .cx
        .update(|app| view.read(app).model.active().unwrap().bookmarked_lines());
    assert_eq!(bm, vec![2], "bookmark follows content down");
    cx.dispatch_action(Undo);
    let (text, bm) = cx.cx.update(|app| {
        let tab = view.read(app).model.active().unwrap();
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
        let tab = ws.model.active().unwrap();
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
    cx.update_window_entity(&view, |ws, _w, _| ws.model.active_mut().unwrap().toggle_fold(0));
    let grid = MouseGrid::of(cx, &view);
    // Клик по «второй видимой строке» — это real-строка 3 ("}"),
    // хотя до свёртывания на этом месте была строка 1.
    cx.simulate_click(grid.at(1.5, 1.2), Modifiers::default());
    let line = cx.cx.update(|app| view.read(app).model.active().unwrap().cursor_line());
    assert_eq!(line, 3);
}

/// 4.5: Cmd+G на скрытую строку — каретка на заголовок фолда.
#[gpui::test]
fn goto_hidden_line_jumps_to_fold_header(cx: &mut TestAppContext) {
    let code = "fn a() {\n    b();\n    c();\n}\ntail\n";
    let (view, cx, _path) = editor_with_file(cx, "fold-goto", code);
    cx.update_window_entity(&view, |ws, _w, _| ws.model.active_mut().unwrap().toggle_fold(0));
    cx.dispatch_action(GoToLine);
    cx.simulate_keystrokes("2 enter"); // строка 2 скрыта телом фолда 0
    let line = cx.cx.update(|app| view.read(app).model.active().unwrap().cursor_line());
    assert_eq!(line, 0, "caret lands on the fold header");
}

/// 4.6: операции строк через действия меню (SortAsc на выделении).
#[gpui::test]
fn sort_lines_action_on_selection(cx: &mut TestAppContext) {
    let (view, cx, _path) = editor_with_file(cx, "sort", "h\npear\napple\nbanana\nf");
    cx.update_window_entity(&view, |ws, _w, _| {
        let tab = ws.model.active_mut().unwrap();
        tab.set_caret_at(1, 0);
        tab.select_to(3, 3); // строки 1..=3
    });
    cx.dispatch_action(SortAsc);
    let text = cx.cx.update(|app| view.read(app).model.active().unwrap().doc.text());
    assert_eq!(text, "h\napple\nbanana\npear\nf");
    // Undo через действие — операция шла через History.
    cx.dispatch_action(Undo);
    let text = cx.cx.update(|app| view.read(app).model.active().unwrap().doc.text());
    assert_eq!(text, "h\npear\napple\nbanana\nf");
}

/// D11.8 (панель PERFORMANCE): секция отрисована в левой панели,
/// кадры render-прохода собираются, opens непусты после открытия.
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
    assert!(cx.cx.update(|app| view.read(app).model.is_empty()));
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
                view.read(app).fold_view,
                Some((_, crate::workspace::FoldView::Identity))
            )
        })
    };
    let visible = |cx: &gpui::VisualTestContext| {
        cx.cx.update(|app| {
            let ws = view.read(app);
            (ws.visible_len(), ws.model.active().unwrap().doc.len_lines())
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
        cx.cx.update(|app| match &view.read(app).fold_view {
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
        ws.model.active_mut().unwrap().toggle_fold(0);
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
            matches!(ws.fold_view, Some((_, crate::workspace::FoldView::Mapped { .. }))),
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
        ws
    });
    cx.focus(&view);
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
        ws.refresh_all_languages();
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
        ws.refresh_all_languages();
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
        ws.refresh_all_languages();
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
    let status = cx.cx.update(|app| view.read(app).status_bar_text());
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
        ws.refresh_all_languages();
        cx.notify();
    });
    cx.run_until_parked();
    let status = cx.cx.update(|app| view.read(app).status_bar_text());
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
    let status = cx.cx.update(|app| view.read(app).status_bar_text());
    assert!(
        status.contains("Syntax highlighting is off"),
        "got: {status}"
    );
    // Редактирование полноценно: ввод работает, документ меняется.
    cx.simulate_keystrokes("z");
    let text_head = cx.cx.update(|app| {
        view.read(app).model.active().unwrap().doc.slice_str(0, 1)
    });
    assert_eq!(text_head, "z");
    cx.dispatch_action(Undo); // cmd-z биндинг в gpui::test пуст — действием
    cx.run_until_parked();
    let text_head = cx.cx.update(|app| {
        view.read(app).model.active().unwrap().doc.slice_str(0, 1)
    });
    assert_eq!(text_head, "a", "undo работает");
    // Подсветки нет: движок без синтаксиса (сегменты может резать
    // только каретка — после ввода она в col 1). Язык — Plain Text.
    let (syntax, label) = cx.cx.update(|app| {
        let tab = view.read(app).model.active().unwrap();
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
        let tab = view.read(app).model.active().unwrap();
        (tab.language_label().to_string(), tab.highlight.syntax_name().map(str::to_string))
    });
    assert_eq!(label, "Rust", "язык определился после согласия");
    assert_eq!(syntax.as_deref(), Some("Rust"), "движок активен для 20 МБ+");
    // Статус-бар сообщает о включении; редактирование работает.
    let status = cx.cx.update(|app| view.read(app).status_bar_text());
    assert!(status.contains("enabled for large"), "got: {status}");
    cx.simulate_keystrokes("z");
    let head = cx.cx.update(|app| {
        view.read(app).model.active().unwrap().doc.slice_str(0, 1)
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
        ws.refresh_all_languages();
        cx.notify();
    });
    cx.run_until_parked();
    let status = cx.cx.update(|app| view.read(app).status_bar_text());
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
        ws.refresh_all_languages();
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
        ws.scroll.scroll_to_item(60, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    cx.run_until_parked();
    cx.update_window_entity(&view, |_ws, _w, cx| cx.notify());
    cx.run_until_parked();
    let header = cx.debug_bounds("csv-sticky-header").expect("шапка закреплена наверху");
    // Ширина ≈ ширине редактора (оверлей на всю ширину контейнера),
    // высота = строке.
    let editor = cx
        .update_window_entity(&view, |ws, w, _| ws.editor_bounds().expect("editor bounds"));
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
        f32::from(ws.scroll.0.borrow().base_handle.offset().y)
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
        ws.scroll.scroll_to_item(60, gpui::ScrollStrategy::Top);
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
        ws.refresh_all_languages();
        cx.notify();
    });
    cx.run_until_parked();
    // Частичный показ строки 0 (−половина строки) — дубля нет.
    cx.update_window_entity(&view, |ws, _w, cx| {
        ws.scroll
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
        ws.scroll
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
