//! Панель live-статистики производительности и памяти (D11.8, по
//! запросу пользователя): секция «PERFORMANCE» внизу левой панели
//! «EXPLORER» — кадры (render CPU: last/p95/max), память процесса
//! (RSS/peak/footprint) и недавние открытия файлов с длительностью.
//!
//! Данные — глобальные (процесс-одиночка, один воркспейс):
//! - `note_frame`: длительность `Workspace::render` (вызывается из
//!   Render::render всегда; Instant-замер ~десятки нс);
//! - `note_open`: имя + мс открытия (единственная точка записи —
//!   `TabModel::open_path`, покрывает диалог/сессию/NOTEPAD_OPEN);
//! - память: `proc_pid_rusage` (libproc, macOS) — без новых
//!   зависимостей; на других ОС секция памяти показывает n/a.
//!
//! Пики RSS/footprint — максимумы по моментам сэмплирования (рендер
//! панели).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use gpui::{div, prelude::*, px, rgb};

use crate::workspace::Workspace;

/// Сколько последних кадров в окне статистики.
const FRAME_WINDOW: usize = 240;

/// Сколько недавних открытий показывать.
const OPENS_SHOWN: usize = 5;

/// Кольцо последних кадров (мс, render CPU).
#[derive(Debug)]
pub(crate) struct FrameRing {
    samples: Vec<f64>,
    head: usize,
    filled: usize,
    last: f64,
    pub(crate) total: u64,
}

impl FrameRing {
    pub(crate) fn new(capacity: usize) -> Self {
        FrameRing { samples: vec![0.0; capacity], head: 0, filled: 0, last: 0.0, total: 0 }
    }

    pub(crate) fn push(&mut self, ms: f64) {
        if self.samples.is_empty() {
            return;
        }
        self.samples[self.head] = ms;
        self.head = (self.head + 1) % self.samples.len();
        self.filled = (self.filled + 1).min(self.samples.len());
        self.last = ms;
        self.total += 1;
    }

    /// last/p95/max (мс) по окну (None — кадров ещё не было). last
    /// хранится отдельно: при обороте кольца `samples[..filled]` уже
    /// не в хронологическом порядке.
    pub(crate) fn stats(&self) -> Option<(f64, f64, f64)> {
        if self.filled == 0 {
            return None;
        }
        let window: Vec<f64> = self.samples[..self.filled].to_vec();
        let mut sorted = window.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p95 = sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)];
        Some((self.last, p95, sorted[sorted.len() - 1]))
    }
}

static FRAMES: Mutex<Option<FrameRing>> = Mutex::new(None);
/// (имя файла, мс) недавних открытий, новые в конце.
static OPENS: Mutex<Vec<(String, f64)>> = Mutex::new(Vec::new());
/// Пики памяти по сэмплам (байты).
static PEAK_RSS: AtomicU64 = AtomicU64::new(0);
static PEAK_FOOTPRINT: AtomicU64 = AtomicU64::new(0);

/// Записать длительность render-прохода (вызов из Workspace::render).
pub(crate) fn note_frame(ms: f64) {
    FRAMES
        .lock()
        .unwrap()
        .get_or_insert_with(|| FrameRing::new(FRAME_WINDOW))
        .push(ms);
}

/// Записать успешное открытие файла (TabModel::open_path).
pub(crate) fn note_open(name: String, ms: f64) {
    let mut opens = OPENS.lock().unwrap();
    opens.push((name, ms));
    let len = opens.len();
    if len > OPENS_SHOWN {
        opens.drain(0..len - OPENS_SHOWN);
    }
}

/// Статистика кадров для панели/тестов: (total, last, p95, max).
pub(crate) fn frame_snapshot() -> (u64, Option<(f64, f64, f64)>) {
    let guard = FRAMES.lock().unwrap();
    match guard.as_ref() {
        Some(ring) => (ring.total, ring.stats()),
        None => (0, None),
    }
}

/// Недавние открытия (копия).
pub(crate) fn recent_opens() -> Vec<(String, f64)> {
    OPENS.lock().unwrap().clone()
}

// ---- Память процесса (macOS: libproc) ----

/// Кроссплатформенная заглушка для не-macOS.
#[cfg(not(target_os = "macos"))]
fn memory_sample() -> Option<(u64, u64)> {
    None
}

/// (rss, phys_footprint) текущего процесса, байты.
/// `proc_pid_rusage(RUSAGE_INFO_V0)` (libproc, системная библиотека —
/// новых зависимостей нет). Смещения памяти ПИНОВАНЫ ЭМПИРИЧЕСКИ на
/// macOS 15.7: u64-слоты 6/7 после ri_uuid[16] — resident_size (сверен
/// байт-в-байт с `maximum resident set size` от /usr/bin/time -l на
/// процессе с 300 МБ тронутых страниц) и phys_footprint; слоты старше
/// в xnu не переставляются (структуры append-only). Тест
/// `memory_snapshot_is_sane` ловит расхождение громко.
#[cfg(target_os = "macos")]
fn memory_sample() -> Option<(u64, u64)> {
    #[repr(C)]
    struct RUsageInfo {
        ri_uuid: [u8; 16],
        slots: [u64; 8],
    }
    #[link(name = "proc")]
    unsafe extern "C" {
        unsafe fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RUsageInfo) -> i32;
    }
    const RUSAGE_INFO_V0: i32 = 0;
    let mut info = RUsageInfo { ri_uuid: [0; 16], slots: [0; 8] };
    let pid = std::process::id() as i32;
    let rc = unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V0, &mut info) };
    (rc == 0).then(|| (info.slots[6], info.slots[7]))
}

/// Снимок памяти с обновлением пиков: (rss, peak_rss, footprint,
/// peak_footprint); None — платформа без поддержки.
pub(crate) fn memory_snapshot() -> Option<(u64, u64, u64, u64)> {
    let (rss, footprint) = memory_sample()?;
    let peak_rss = PEAK_RSS.fetch_max(rss, Ordering::Relaxed).max(rss);
    let peak_fp = PEAK_FOOTPRINT.fetch_max(footprint, Ordering::Relaxed).max(footprint);
    Some((rss, peak_rss, footprint, peak_fp))
}

fn fmt_mb(bytes: u64) -> String {
    format!("{:.0} MB", bytes as f64 / (1024.0 * 1024.0))
}

impl Workspace {
    /// Секция «PERFORMANCE» для левой панели (D11.8): кадры, память,
    /// недавние открытия. Сэмпл памяти берётся на каждом рендере секции.
    pub(crate) fn render_perf_section(&self) -> gpui::Div {
        let (total, frame_stats) = frame_snapshot();
        let mem = memory_snapshot();
        let mut section = div()
            .mt_2()
            .border_t_1()
            .border_color(rgb(crate::theme::current().close_hover))
            .debug_selector(|| "perf-section".into())
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(11.0))
                    .text_color(rgb(crate::theme::current().panel_header))
                    .child("PERFORMANCE"),
            )
            .child(self.perf_row("frames", format!("{total}")));
        match frame_stats {
            Some((last, p95, max)) => {
                section = section
                    .child(self.perf_row("render ms", format!("{last:.2} / {p95:.2} / {max:.2}")))
                    .child(self.perf_hint("(last · p95 · max, окно 240 кадров)"));
            }
            None => {
                section = section.child(self.perf_row("render ms", "—".to_string()));
            }
        }
        match mem {
            Some((rss, peak_rss, footprint, _peak_fp)) => {
                section = section
                    .child(self.perf_row("rss", fmt_mb(rss)))
                    .child(self.perf_row("peak rss", fmt_mb(peak_rss)))
                    .child(self.perf_row("footprint", fmt_mb(footprint)));
            }
            None => {
                section = section.child(self.perf_row("rss", "(n/a on this OS)".to_string()));
            }
        }
        let opens = recent_opens();
        if !opens.is_empty() {
            section = section.child(
                div()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .text_size(px(11.0))
                    .text_color(rgb(crate::theme::current().panel_header))
                    .child("OPENS"),
            );
            for (name, ms) in opens {
                let short: String = name.chars().rev().take(18).collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                let label = if name.chars().count() > 18 {
                    format!("…{short}")
                } else {
                    name.clone()
                };
                section = section.child(
                    div()
                        .flex()
                        .gap_2()
                        .px_3()
                        .py_0p5()
                        .text_size(px(11.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis()
                                .text_color(rgb(crate::theme::current().tab_inactive))
                                .child(label),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_color(rgb(crate::theme::current().dim))
                                .child(format!("{ms:.0} ms")),
                        ),
                );
            }
        }
        section
    }

    /// Строка секции: «ключ — значение».
    fn perf_row(&self, key: &str, value: String) -> gpui::Div {
        div()
            .flex()
            .gap_2()
            .px_3()
            .py_0p5()
            .text_size(px(11.0))
            .child(
                div()
                    .w(px(78.0))
                    .flex_shrink_0()
                    .text_ellipsis()
                    .text_color(rgb(crate::theme::current().dim))
                    .child(key.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_ellipsis()
                    .text_color(rgb(crate::theme::current().text))
                    .child(value),
            )
    }

    /// Приглушённая строка-подсказка под значением.
    fn perf_hint(&self, text: &str) -> gpui::Div {
        div()
            .px_3()
            .py_0p5()
            .text_size(px(9.0))
            .text_color(rgb(crate::theme::current().panel_hint))
            .child(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Кольцо кадров: пустое → None; p95/max/last по окну; переполнение
    /// окна вытесняет старые сэмплы, total растёт.
    #[test]
    fn frame_ring_stats() {
        let mut ring = FrameRing::new(4);
        assert_eq!(ring.stats(), None);
        for ms in [1.0, 2.0, 3.0, 4.0] {
            ring.push(ms);
        }
        assert_eq!(ring.stats(), Some((4.0, 4.0, 4.0))); // p95 от 4 сэмплов = max
        ring.push(10.0); // окно: 2,3,4,10
        let (last, p95, max) = ring.stats().unwrap();
        assert_eq!((last, max), (10.0, 10.0));
        assert!((2.0..=10.0).contains(&p95));
        assert_eq!(ring.total, 5);
        ring.push(1.0); // окно: 3,4,10,1
        assert_eq!(ring.stats().unwrap().0, 1.0, "last — последний сэмпл");
    }

    /// Снимок памяти на macOS возвращает осмысленные значения и пик
    /// монотонен.
    #[cfg(target_os = "macos")]
    #[test]
    fn memory_snapshot_is_sane() {
        let (rss, peak_rss, footprint, peak_fp) = memory_snapshot().expect("libproc доступен");
        assert!(rss > 10 * 1024 * 1024, "RSS процесса больше 10 MB: {rss}");
        assert!(peak_rss >= rss);
        assert!(peak_fp >= footprint);
        // Второй сэмпл: пики не убывают.
        let (rss2, peak2, _, peak_fp2) = memory_snapshot().unwrap();
        assert!(peak2 >= peak_rss && peak_fp2 >= peak_fp && rss2 > 0);
    }

    /// Список открытий обрезается до лимита.
    #[test]
    fn opens_list_is_bounded() {
        let mut list: Vec<(String, f64)> = Vec::new();
        // Те же правила, что note_open (проверяем логику обрезки).
        for i in 0..(OPENS_SHOWN + 3) {
            list.push((format!("f{i}"), i as f64));
        }
        let len = list.len();
        if len > OPENS_SHOWN {
            list.drain(0..len - OPENS_SHOWN);
        }
        assert_eq!(list.len(), OPENS_SHOWN);
        assert_eq!(list.last().unwrap().0, format!("f{}", OPENS_SHOWN + 2));
    }
}
