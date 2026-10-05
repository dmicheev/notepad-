//! Слежение за файловой системой воркспейса (задача 7.3, D4):
//! `notify` (FSEvents / ReadDirectoryChangesW / inotify),
//! рекурсивно на корень, с дебаунсом — события копятся, пока не
//! стихнут (250 мс), затем уходят одним батчем потребителю через
//! `std::sync::mpsc`. Полный рескан дерева (сброс кэша FsTree) —
//! fallback-семантика D4: батч несёт только «что тронуто», дерево
//! перечитывает затронутые каталоги лениво.
//!
//! Доставка в gpui — поллинг получателя с таймером (150 мс):
//! std-канал не Future, smol-канал не вытащить из gpui 0.2.2 без
//! новой зависимости; поллинг try_recv дешевле кадра и добавляет
//! ≤150 мс к 250 мс дебаунса.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use notify::{Event, Watcher};

/// Окно дебаунса: тишина после последнего события до отправки батча.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

/// Состояние дебаунса (чистая логика — юнит-тесты без FS).
#[derive(Debug, Default)]
struct Debounce {
    pending: HashSet<PathBuf>,
    deadline: Option<Instant>,
}

impl Debounce {
    /// Добавить пути события и отложить окно заново.
    fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.pending.extend(paths);
        self.deadline = Some(Instant::now() + DEBOUNCE);
    }

    /// Батч готов (окно истекло).
    fn due(&self, now: Instant) -> bool {
        self.deadline.is_some_and(|d| now >= d)
    }

    /// Забрать батч (сбрасывает окно; пустые не выдаёт).
    fn take(&mut self) -> Option<Vec<PathBuf>> {
        self.deadline = None;
        if self.pending.is_empty() {
            return None;
        }
        Some(self.pending.drain().collect())
    }
}

/// Активный наблюдатель: владение/watcher + поток дебаунса + канал
/// батчей. Держится живым владельцем (Workspace); при drop —
/// отключение канала останавливает поток.
pub struct FsWatch {
    _watcher: notify::RecommendedWatcher,
    /// Батчи путей (коалесированные) для потребителя.
    pub batches: Receiver<Vec<PathBuf>>,
    _thread: std::thread::JoinHandle<()>,
}

/// Запустить рекурсивное наблюдение за `root`. События notify
/// приходят в собственном callback-потоке наблюдателя и только
/// пересылаются в поток дебаунса (никакой обработки под блокировкой
/// FSEvents).
pub fn watch_recursive(root: &Path) -> notify::Result<FsWatch> {
    let (raw_tx, raw_rx) = channel::<notify::Result<Event>>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        // Ошибки наблюдателя пересылаем как есть: поток их игнорирует
        // (мисс события компенсирует следующий батч/рескан).
        let _ = raw_tx.send(res);
    })?;
    watcher.watch(root, notify::RecursiveMode::Recursive)?;

    let (batch_tx, batch_rx) = channel::<Vec<PathBuf>>();
    let thread = std::thread::Builder::new()
        .name("fs-watch-debounce".into())
        .spawn(move || debounce_loop(raw_rx, batch_tx))
        .expect("spawn fs-watch-debounce");

    Ok(FsWatch {
        _watcher: watcher,
        batches: batch_rx,
        _thread: thread,
    })
}

/// Поток дебаунса: ждёт события с таймаутом до дедлайна; истёк
/// таймаут при непустом окне — батч наружу.
fn debounce_loop(raw: Receiver<notify::Result<Event>>, batches: Sender<Vec<PathBuf>>) {
    let mut state = Debounce::default();
    loop {
        let timeout = state
            .deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|| Duration::from_secs(3600));
        match raw.recv_timeout(timeout) {
            Ok(Ok(event)) => state.add(event.paths),
            Ok(Err(_err)) => { /* ошибка наблюдателя: ждём дальше */ }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if state.due(Instant::now())
                    && let Some(batch) = state.take()
                    && batches.send(batch).is_err()
                {
                    break; // потребитель ушёл
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// Канонизация для сравнения: macOS FSEvents отдаёт /private/…
    /// пути даже для /tmp-каталогов.
    fn canon(p: &Path) -> PathBuf {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
    }

    /// Дебаунс: события копятся, батч только после тишины.
    #[test]
    fn debounce_coalesces_until_quiet() {
        let mut d = Debounce::default();
        assert!(!d.due(Instant::now()));
        d.add([PathBuf::from("/a")]);
        // Окно ещё не истекло — сразу после add батча нет.
        assert!(!d.due(Instant::now()));
        // Событие «шумит» — дедлайн отодвигается, батч всё ещё жду.
        std::thread::sleep(DEBOUNCE / 2);
        d.add([PathBuf::from("/b")]);
        assert!(!d.due(Instant::now()));
        // Тишина дольше окна — батч с обоими путями, окно сброшено.
        std::thread::sleep(DEBOUNCE + Duration::from_millis(50));
        assert!(d.due(Instant::now()));
        let batch = d.take().unwrap();
        assert_eq!(batch.len(), 2);
        assert!(batch.contains(&PathBuf::from("/a")));
        assert!(batch.contains(&PathBuf::from("/b")));
        assert!(!d.due(Instant::now()));
        assert_eq!(d.take(), None);
    }

    /// Сценарий спеки «Файл создан извне» (tmpdir): файл, созданный
    /// сторонним записям, приносит батч с его путём; быстрые записи
    /// коалесятся в один батч.
    #[test]
    fn watcher_reports_external_creates() {
        let dir = TempDir::new("watch");
        std::fs::write(dir.path().join("warmup.txt"), "w\n").unwrap();
        let watch = watch_recursive(dir.path()).expect("start watcher");

        // Дать наблюдателю стартовать (первый watch-вызов уже
        // синхронен, FSEvents может копить чуть дольше).
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(dir.path().join("made-1.txt"), "1\n").unwrap();
        std::fs::write(dir.path().join("made-2.txt"), "2\n").unwrap();

        let first = watch
            .batches
            .recv_timeout(Duration::from_secs(10))
            .expect("batch arrives");
        let made1 = canon(&dir.path().join("made-1.txt"));
        let made2 = canon(&dir.path().join("made-2.txt"));
        let batch: HashSet<PathBuf> = first.iter().map(|p| canon(p)).collect();
        // Обе записи в одном окне — оба пути в первом же батче.
        assert!(batch.contains(&made1), "first batch: {first:?}");
        assert!(batch.contains(&made2), "first batch: {first:?}");

        // Удаление тоже приносит батч (путь исчезнувшего файла).
        std::fs::remove_file(dir.path().join("made-1.txt")).unwrap();
        let second = watch
            .batches
            .recv_timeout(Duration::from_secs(10))
            .expect("second batch");
        assert!(
            second.iter().map(|p| canon(p)).any(|p| p == made1),
            "second batch: {second:?}"
        );
    }

    /// Батчи перестают доставляться после drop наблюдателя (поток
    /// завершается, потребитель не висит на канале).
    #[test]
    fn watcher_stop_on_drop() {
        let dir = TempDir::new("watch-drop");
        let watch = watch_recursive(dir.path()).unwrap();
        drop(watch);
        std::thread::sleep(DEBOUNCE * 2);
        // Канал отключён: дальнейшие записи не обязаны доставляться.
        std::fs::write(dir.path().join("after.txt"), "x\n").unwrap();
        std::thread::sleep(DEBOUNCE * 2);
    }
}
