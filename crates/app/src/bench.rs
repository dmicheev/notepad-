//! Бенчмарк-стенд (D11.1, design.md): env-режим `NOTEPAD_BENCH=1`.
//!
//! При старте открывает файл из `NOTEPAD_OPEN` и печатает в stderr
//! тайминги: открытие (load_file_detected в составе BufferTab::open),
//! первый кадр после открытия, N кадров скролла (программные
//! `scroll_to_item` по псевдослучайным строкам, детерминированный
//! LCG), тайпинг M символов через модель (GPUI-free путь
//! `BufferTab::insert_text`), undo/redo всей группы ввода.
//!
//! Драйвер — async-задача с таймером (~16 мс между шагами); каждый
//! шаг делает действие (скролл/ввод/undo) и строит кадр ручным
//! `Window::draw` (layout+paint видимых строк — процессор
//! uniform_list и есть объект замера). Ручной draw нужен потому, что
//! на macOS перерисовка gpui идёт через CVDisplayLink, который при
//! запуске из CLI может не стартовать (окно неактивно) — стенд не
//! должен зависеть от платформенного vsync. Метрика шага
//! «action_to_frame_ms» = действие + notify + полный draw — это и есть
//! бюджет «ввод < 8 мс до кадра». Длительности сборки видимых строк
//! (процессор uniform_list) собираются отдельно, по фазам.
//!
//! Пиковую память процесса меряет внешний запуск:
//! `/usr/bin/time -l ./notepad-plus`.
//!
//! Управление: `NOTEPAD_BENCH_SCROLL=N` (по умолчанию 100),
//! `NOTEPAD_BENCH_TYPE=M` (по умолчанию 200). Завершение —
//! `std::process::exit(0)` (мимо on_app_quit, сессию не трогает).

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::{App, AsyncApp, Context, ScrollStrategy, Window, WindowHandle};

use crate::workspace::Workspace;

/// Фазы стенда — теги собираемых кадров/шагов.
pub const PHASE_WARMUP: u8 = 1;
pub const PHASE_SCROLL: u8 = 2;
pub const PHASE_TYPING: u8 = 3;
pub const PHASE_UNDO: u8 = 4;

/// Пауза между шагами (мс): имитация темпа интеракции, не даёт
/// соседним кадрам сливаться.
const STEP_GAP: Duration = Duration::from_millis(16);

static ENABLED: AtomicBool = AtomicBool::new(false);
static PHASE: AtomicU8 = AtomicU8::new(PHASE_WARMUP);
static LCG: AtomicU64 = AtomicU64::new(0x0DA4_4D4C_B7C2_1657);
static LOAD_MICROS: AtomicU64 = AtomicU64::new(0);
static UNDO_MICROS: AtomicU64 = AtomicU64::new(0);
static REDO_MICROS: AtomicU64 = AtomicU64::new(0);
static LOAD_END: OnceLock<Instant> = OnceLock::new();
static FIRST_FRAME: OnceLock<Instant> = OnceLock::new();
/// Момент main() — точка отсчёта холодного старта (бюджет «стартап с
/// сессией из 200 МБ-вкладки»: NOTEPAD_OPEN проксирует восстановление
/// сессии — то же синхронное открытие вкладки до первого кадра).
static START: OnceLock<Instant> = OnceLock::new();
static TYPE_US: Mutex<Vec<f64>> = Mutex::new(Vec::new());
static FRAMES: Mutex<Vec<(u8, f64)>> = Mutex::new(Vec::new());
static STEPS: Mutex<Vec<(u8, f64)>> = Mutex::new(Vec::new());
/// Полное время ручного draw (layout+paint всего кадра, включая
/// ensure_fold_view и процессор uniform_list) — в окружениях без
/// платформенных кадров; honest-метрика «действие → кадр построен».
static DRAWS: Mutex<Vec<(u8, f64)>> = Mutex::new(Vec::new());
/// Длительность Workspace::render (ensure_fold_view + построение
/// элементного дерева) — CPU-стоимость кадра, не зависящая от vsync.
static RENDERS: Mutex<Vec<(u8, f64)>> = Mutex::new(Vec::new());
/// Латентность «действие завершено → кадр отрисован» (natural —
/// оценка сверху: ожидание vsync + гранулярность опроса 5 мс;
/// manual-путь включает ожидание 50 мс + draw — узнаётся по ~50+ мс).
static LATS: Mutex<Vec<(u8, f64)>> = Mutex::new(Vec::new());
/// Монотонный счётчик кадров (детект «платформа рисует сама»).
static FRAME_SEQ: AtomicU64 = AtomicU64::new(0);

/// Включить стенд по env (`NOTEPAD_BENCH=1`); вызывается из main.
pub fn enable_from_env() {
    let _ = START.set(Instant::now());
    if std::env::var("NOTEPAD_BENCH").ok().as_deref() == Some("1") {
        ENABLED.store(true, Ordering::Relaxed);
    }
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

fn env_count(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Записать время открытия (BufferTab::open) и момент его окончания
/// (точка отсчёта первого кадра).
pub fn record_load(d: Duration) {
    LOAD_MICROS.store(d.as_micros() as u64, Ordering::Relaxed);
    let _ = LOAD_END.set(Instant::now());
}

/// Первый рендер Workspace (OnceLock хранит только первое срабатывание).
pub fn record_first_frame() {
    let _ = FIRST_FRAME.set(Instant::now());
}

/// Кадр процессора uniform_list (editor_view), тег — текущая фаза.
pub fn record_frame(ms: f64) {
    if enabled() {
        FRAME_SEQ.fetch_add(1, Ordering::Relaxed);
        FRAMES
            .lock()
            .unwrap()
            .push((PHASE.load(Ordering::Relaxed), ms));
    }
}

/// Длительность Workspace::render (CPU-стоимость кадра).
pub fn record_render(ms: f64) {
    if enabled() {
        RENDERS
            .lock()
            .unwrap()
            .push((PHASE.load(Ordering::Relaxed), ms));
    }
}

/// Запустить драйвер (после создания окна; первый кадр open_window
/// уже отрисован — момент фиксирует record_first_frame).
pub fn start(window: WindowHandle<Workspace>, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        run(window, cx).await;
    })
    .detach();
}

async fn run(window: WindowHandle<Workspace>, cx: &mut AsyncApp) {
    let scrolls = env_count("NOTEPAD_BENCH_SCROLL", 100);
    let types = env_count("NOTEPAD_BENCH_TYPE", 200);

    // Разогрев: кэши шрифтов/шейдеров не попадают в статистику.
    PHASE.store(PHASE_WARMUP, Ordering::Relaxed);
    for _ in 0..15 {
        step(&window, cx, |_, cx| cx.notify());
        frame(&window, cx).await;
        cx.background_executor().timer(STEP_GAP).await;
    }

    // Скролл: прыжки по псевдослучайным видимым строкам.
    PHASE.store(PHASE_SCROLL, Ordering::Relaxed);
    for _ in 0..scrolls {
        step(&window, cx, |ws, cx| {
            ws.ensure_fold_view();
            let len = ws.visible_len().max(1);
            let target = (next_random() % len as u64) as usize;
            ws.panes[0].scroll.scroll_to_item(target, ScrollStrategy::Center);
            cx.notify();
        });
        frame(&window, cx).await;
        cx.background_executor().timer(STEP_GAP).await;
    }

    // Тайпинг: одиночные вставки через модель (путь apply_tx + History,
    // как при живом вводе) + кадр на каждый символ.
    PHASE.store(PHASE_TYPING, Ordering::Relaxed);
    for _ in 0..types {
        step(&window, cx, |ws, cx| {
            let t = Instant::now();
            if let Some(tab) = ws.model_mut().active_mut() {
                tab.insert_text("x");
            }
            cx.notify();
            TYPE_US
                .lock()
                .unwrap()
                .push(t.elapsed().as_secs_f64() * 1e6);
        });
        frame(&window, cx).await;
        cx.background_executor().timer(STEP_GAP).await;
    }

    // Undo/redo всей группы ввода одним вызовом (политика группировки
    // History: серия одиночных вставок — одна группа).
    PHASE.store(PHASE_UNDO, Ordering::Relaxed);
    for op in 0..2 {
        step(&window, cx, |ws, cx| {
            let t = Instant::now();
            if let Some(tab) = ws.model_mut().active_mut() {
                if op == 0 {
                    tab.undo();
                } else {
                    tab.redo();
                }
            }
            cx.notify();
            let us = t.elapsed().as_micros() as u64;
            if op == 0 {
                UNDO_MICROS.store(us, Ordering::Relaxed);
            } else {
                REDO_MICROS.store(us, Ordering::Relaxed);
            }
        });
        frame(&window, cx).await;
        cx.background_executor().timer(STEP_GAP).await;
    }

    report();
    std::process::exit(0);
}

/// Один шаг стенда: действие (в контексте сущности Workspace) → notify
/// → кадр. Кадр — естественный (платформенный vsync), если платформа
/// рисует сама; если за 50 мс кадра нет (фоновый процесс без
/// CVDisplayLink — запуск из CLI), кадр строится ручным `Window::draw`
/// (layout+paint видимых строк; present/GPU-сторона не входит в метрику
/// в обоих случаях). Ручной draw НЕ совмещается с платформенным: в
/// активном окне платформа даёт кадр за ~16 мс и ручной путь не
/// используется (иначе двойная аренда корневой сущности — крэш
/// double_lease_panic).
fn step(
    window: &WindowHandle<Workspace>,
    cx: &mut AsyncApp,
    action: impl FnOnce(&mut Workspace, &mut Context<Workspace>),
) {
    let t0 = Instant::now();
    let _ = window.update(cx, |ws, _window, cx| action(ws, cx));
    STEPS
        .lock()
        .unwrap()
        .push((PHASE.load(Ordering::Relaxed), t0.elapsed().as_secs_f64() * 1000.0));
}

/// Ждать естественный кадр; при его отсутствии (50 мс) — построить
/// вручную. Вызывается после `step` с уже выполненным действием.
async fn frame(window: &WindowHandle<Workspace>, cx: &mut AsyncApp) {
    let t0 = Instant::now();
    let before = FRAME_SEQ.load(Ordering::Relaxed);
    for _ in 0..10 {
        cx.background_executor()
            .timer(Duration::from_millis(5))
            .await;
        if FRAME_SEQ.load(Ordering::Relaxed) != before {
            LATS.lock().unwrap().push((
                PHASE.load(Ordering::Relaxed),
                t0.elapsed().as_secs_f64() * 1000.0,
            ));
            return; // платформа отрисовала кадр сама
        }
    }
    let t = Instant::now();
    let any: gpui::AnyWindowHandle = (*window).into();
    let _ = any.update(cx, |_root, window: &mut Window, cx| {
        let clear = window.draw(cx);
        clear.clear();
    });
    DRAWS
        .lock()
        .unwrap()
        .push((PHASE.load(Ordering::Relaxed), t.elapsed().as_secs_f64() * 1000.0));
}

fn next_random() -> u64 {
    let mut s = LCG.load(Ordering::Relaxed);
    loop {
        let n = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        match LCG.compare_exchange_weak(s, n, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return n,
            Err(v) => s = v,
        }
    }
}

/// Перцентиль отсортированной выборки (nearest-rank).
fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((p / 100.0) * sorted.len() as f64).ceil() as usize - 1;
    sorted[idx.min(sorted.len() - 1)]
}

fn stats_line(label: &str, values: &[f64]) -> String {
    if values.is_empty() {
        return format!("[bench] {label}: no samples");
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    format!(
        "[bench] {label}: n={} mean={:.2} p50={:.2} p95={:.2} max={:.2}",
        values.len(),
        values.iter().sum::<f64>() / values.len() as f64,
        pct(&sorted, 50.0),
        pct(&sorted, 95.0),
        sorted[sorted.len() - 1],
    )
}

fn phase_values(samples: &[(u8, f64)], phase: u8) -> Vec<f64> {
    samples.iter().filter(|(p, _)| *p == phase).map(|(_, v)| *v).collect()
}

fn report() {
    eprintln!("[bench] load_ms={:.1}", LOAD_MICROS.load(Ordering::Relaxed) as f64 / 1000.0);
    if let (Some(end), Some(frame)) = (LOAD_END.get(), FIRST_FRAME.get()) {
        eprintln!(
            "[bench] first_frame_after_load_ms={:.1}",
            frame.duration_since(*end).as_secs_f64() * 1000.0
        );
    }
    if let (Some(start), Some(frame)) = (START.get(), FIRST_FRAME.get()) {
        eprintln!(
            "[bench] cold_start_to_first_frame_ms={:.1}",
            frame.duration_since(*start).as_secs_f64() * 1000.0
        );
    }
    let frames = FRAMES.lock().unwrap().clone();
    for (phase, name) in [
        (PHASE_SCROLL, "scroll_frame_ms"),
        (PHASE_TYPING, "typing_frame_ms"),
        (PHASE_UNDO, "undo_frame_ms"),
    ] {
        eprintln!("{}", stats_line(name, &phase_values(&frames, phase)));
    }
    let steps = STEPS.lock().unwrap().clone();
    for (phase, name) in [
        (PHASE_SCROLL, "scroll_action_ms"),
        (PHASE_TYPING, "typing_action_ms"),
        (PHASE_UNDO, "undo_action_ms"),
    ] {
        eprintln!("{}", stats_line(name, &phase_values(&steps, phase)));
    }
    let draws = DRAWS.lock().unwrap().clone();
    for (phase, name) in [
        (PHASE_SCROLL, "scroll_full_frame_ms"),
        (PHASE_TYPING, "typing_full_frame_ms"),
        (PHASE_UNDO, "undo_full_frame_ms"),
    ] {
        eprintln!("{}", stats_line(name, &phase_values(&draws, phase)));
    }
    let renders = RENDERS.lock().unwrap().clone();
    for (phase, name) in [
        (PHASE_SCROLL, "scroll_render_ms"),
        (PHASE_TYPING, "typing_render_ms"),
        (PHASE_UNDO, "undo_render_ms"),
    ] {
        eprintln!("{}", stats_line(name, &phase_values(&renders, phase)));
    }
    let lats = LATS.lock().unwrap().clone();
    for (phase, name) in [
        (PHASE_SCROLL, "scroll_step_to_frame_ms"),
        (PHASE_TYPING, "typing_step_to_frame_ms"),
    ] {
        eprintln!("{}", stats_line(name, &phase_values(&lats, phase)));
    }
    eprintln!("{}", stats_line("typing_model_us", &TYPE_US.lock().unwrap().clone()));
    eprintln!("[bench] undo_ms={:.1}", UNDO_MICROS.load(Ordering::Relaxed) as f64 / 1000.0);
    eprintln!("[bench] redo_ms={:.1}", REDO_MICROS.load(Ordering::Relaxed) as f64 / 1000.0);
}
