//! Фоновый поиск Tabs/Folder (группа 6, D13): запуск сканов по вкладкам
//! и папке, потоковые батчи из background executor, применение результатов
//! и замена по файлам (в память вкладок). Состояние панели и File-scope —
//! search_panel.rs; рендер — search_render.rs.

use std::path::PathBuf;

use editor_core::ScanSummary;
use gpui::{Context, PromptLevel, Window};

use crate::search_panel::{
    MAX_HITS_PER_TAB, MAX_HITS_TOTAL, SearchHit, SearchOwner, SearchScope,
    fragment_from_content, path_hash,
};
use crate::search_panel::ScanEvent;
use crate::workspace::Workspace;

impl Workspace {
    // ---- Tabs scope (6.3) ----

    /// Поиск по всем открытым вкладкам (untitled участвуют — контент
    /// в памяти). ФОНОВЫЙ (баг из ручной проверки: синхронный поиск по
    /// большим вкладкам вешал UI, а неограниченные результаты — сотни
    /// тысяч `SearchHit` с фрагментами — убивали рендер левой панели на
    /// каждом кадре): ropes клонируются (дёшево, дерево ropey) и ищутся
    /// на background executor; результаты капятся (MAX_HITS_PER_TAB /
    /// MAX_HITS_TOTAL) с пометкой усечения.
    fn run_tabs_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sp) = self.search.as_mut() else { return };
        sp.error = None;
        sp.results.clear();
        sp.summary = None;
        let Some(matcher) = Self::search_matcher(sp) else {
            sp.scanning = false;
            cx.notify();
            return;
        };
        let matcher = match matcher {
            Ok(m) => m,
            Err(e) => {
                sp.scanning = false;
                sp.error = Some(e.clone());
                self.status_message = Some(format!("Invalid search pattern: {e}"));
                cx.notify();
                return;
            }
        };
        sp.scanning = true;
        // Данные вкладок для фонового потока (вкладки остаются в модели).
        let tabs: Vec<(usize, String, editor_core::ropey::Rope)> = (0..self.model.len())
            .filter_map(|ix| {
                let label = self.model.title(ix).unwrap_or_default();
                let tab = self.model.tab_mut(ix)?;
                Some((ix, label, tab.doc.rope().clone()))
            })
            .collect();
        let tab_count = tabs.len();
        let search = cx.background_executor().spawn(async move {
            let mut results: Vec<SearchHit> = Vec::new();
            let mut truncated = false;
            'tabs: for (ix, label, rope) in tabs {
                for (per_tab, (s, e)) in matcher.find_in_rope(&rope).into_iter().enumerate() {
                    if results.len() >= MAX_HITS_TOTAL {
                        truncated = true;
                        break 'tabs; // суммарный кап — стоп поиска
                    }
                    if per_tab >= MAX_HITS_PER_TAB {
                        truncated = true;
                        break; // кап вкладки — к следующей вкладке
                    }
                    let line = rope.char_to_line(s);
                    let ls = rope.line_to_char(line);
                    let col = s - ls;
                    let len = e - s;
                    let content = rope.line(line).to_string();
                    let (fragment, frag_col, frag_len) =
                        fragment_from_content(&content, col, len);
                    results.push(SearchHit {
                        owner: SearchOwner::Tab { index: ix, label: label.clone() },
                        line,
                        col,
                        len,
                        fragment,
                        frag_col,
                        frag_len,
                    });
                }
            }
            (results, truncated)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (results, truncated) = search.await;
            this.update_in(cx, |ws, _window, cx| {
                let Some(sp) = ws.search.as_mut() else { return };
                sp.scanning = false;
                sp.truncated = truncated;
                // tabs search: владельцы — только вкладки.
                let files = results
                    .iter()
                    .map(|h| match &h.owner {
                        SearchOwner::Tab { index, .. } => *index,
                        SearchOwner::Path { path, .. } => path_hash(path),
                    })
                    .collect::<std::collections::BTreeSet<_>>()
                    .len();
                sp.summary = Some(ScanSummary {
                    files_scanned: tab_count,
                    files_skipped: 0,
                    files_with_hits: files,
                    total_hits: results.len(),
                });
                sp.results = results;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Enter в Tabs/Folder scope: запуск соответствующего поиска.
    pub(crate) fn run_tabs_or_folder_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scope = self
            .search
            .as_ref()
            .map(|sp| sp.scope.clone())
            .unwrap_or(SearchScope::File);
        match scope {
            SearchScope::Tabs => self.run_tabs_search(window, cx),
            SearchScope::Folder(_) => self.start_folder_scan(window, cx),
            SearchScope::File => {}
        }
    }

    // ---- Folder scope (6.4/6.5) ----

    /// Запуск фонового скана папки: вкладки, чьи пути лежат в папке,
    /// ищутся по ИХ содержимому (спека: повторный поиск видит вкладки,
    /// не диск) — сразу; остальные файлы — editor-core::scan_paths
    /// (memmap+rayon) фоновым executor'ом, батчи через std-канал,
    /// UI-задача поллит (таймер 30 мс) и обновляет панель (UI не
    /// блокируется; в gpui::test таймер ускоряется advance_clock).
    pub(crate) fn start_folder_scan(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(sp) = self.search.as_mut() else { return };
        let SearchScope::Folder(folder) = sp.scope.clone() else {
            return;
        };
        sp.error = None;
        sp.results.clear();
        sp.summary = None;
        let matcher = match Self::search_matcher(sp) {
            None => {
                cx.notify();
                return;
            }
            Some(Ok(m)) => m,
            Some(Err(e)) => {
                sp.error = Some(e.clone());
                self.status_message = Some(format!("Invalid search pattern: {e}"));
                cx.notify();
                return;
            }
        };
        let mask = sp.mask.clone();
        sp.scanning = true;

        // 1. Открытые вкладки в папке — по вкладочному содержимому.
        let mut results = Vec::new();
        let mut open_paths: Vec<PathBuf> = Vec::new();
        for ix in 0..self.model.len() {
            let label = self.model.title(ix).unwrap_or_default();
            let Some(tab) = self.model.tab_mut(ix) else { continue };
            let Some(path) = tab.path.clone() else { continue };
            if !path.starts_with(&folder) {
                continue;
            }
            open_paths.push(path);
            for (s, e) in matcher.find_in_doc(&tab.doc) {
                let line = tab.doc.char_to_line(s);
                let ls = tab.doc.line_to_char(line);
                let content = tab.doc.line_content(line);
                let (fragment, frag_col, frag_len) =
                    fragment_from_content(&content, s - ls, e - s);
                results.push(SearchHit {
                    owner: SearchOwner::Tab { index: ix, label: label.clone() },
                    line,
                    col: s - ls,
                    len: e - s,
                    fragment,
                    frag_col,
                    frag_len,
                });
            }
        }
        if let Some(sp) = self.search.as_mut() {
            sp.results = results;
        }

        // 2. Остальные файлы — фоновый скан.
        let paths = match editor_core::search::collect_files(&folder, &mask) {
            Ok(mut all) => {
                all.retain(|p| !open_paths.contains(p));
                all
            }
            Err(e) => {
                if let Some(sp) = self.search.as_mut() {
                    sp.scanning = false;
                    sp.error = Some(e.to_string());
                }
                cx.notify();
                return;
            }
        };

        let (tx, rx) = std::sync::mpsc::channel::<ScanEvent>();
        let tx_scan = tx.clone();
        cx.background_executor()
            .spawn(async move {
                let summary = editor_core::search::scan_paths(&paths, &matcher, &move |batch| {
                    // Пустые батчи (файлы без совпадений) не шлём — UI
                    // обновляется только по существенным событиям.
                    if !batch.iter().all(|s| s.hits.is_empty()) {
                        let _ = tx_scan.send(ScanEvent::Batch(batch));
                    }
                });
                let _ = tx.send(ScanEvent::Done(summary));
            })
            .detach();

        cx.spawn_in(_window, async move |this, cx| {
            loop {
                let mut done: Option<ScanSummary> = None;
                loop {
                    match rx.try_recv() {
                        Ok(ScanEvent::Batch(batch)) => {
                            this.update(cx, |ws, cx| {
                                ws.apply_search_batch(batch);
                                cx.notify();
                            })
                            .ok();
                        }
                        Ok(ScanEvent::Done(summary)) => {
                            done = Some(summary);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                if let Some(summary) = done {
                    this.update(cx, |ws, cx| {
                        ws.finish_folder_scan(summary);
                        cx.notify();
                    })
                    .ok();
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(30))
                    .await;
            }
        })
        .detach();
        cx.notify();
    }

    /// Вставить батч скана (сортировка результатов для устойчивого UI).
    pub(crate) fn apply_search_batch(&mut self, batch: Vec<editor_core::search::FileScan>) {
        let Some(sp) = self.search.as_mut() else { return };
        for scan in batch {
            if scan.hits.is_empty() {
                continue;
            }
            let label = scan
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| scan.path.display().to_string());
            for hit in scan.hits {
                sp.results.push(SearchHit {
                    owner: SearchOwner::Path { path: scan.path.clone(), label: label.clone() },
                    line: hit.line,
                    col: hit.col,
                    len: hit.len,
                    fragment: hit.fragment,
                    frag_col: hit.frag_col,
                    frag_len: hit.frag_len,
                });
            }
        }
        sp.results.sort_by(|a, b| match (&a.owner, &b.owner) {
            (SearchOwner::Tab { index: i1, .. }, SearchOwner::Tab { index: i2, .. }) => {
                i1.cmp(i2).then(a.line.cmp(&b.line))
            }
            (SearchOwner::Tab { .. }, SearchOwner::Path { .. }) => std::cmp::Ordering::Less,
            (SearchOwner::Path { .. }, SearchOwner::Tab { .. }) => std::cmp::Ordering::Greater,
            (
                SearchOwner::Path { path: p1, .. },
                SearchOwner::Path { path: p2, .. },
            ) => p1.cmp(p2).then(a.line.cmp(&b.line)),
        });
    }

    /// Завершение скана: итог + сообщение статус-бара.
    pub(crate) fn finish_folder_scan(&mut self, summary: ScanSummary) {
        let Some(sp) = self.search.as_mut() else { return };
        sp.scanning = false;
        sp.summary = Some(summary);
        // Открытые вкладки могли дать совпадения вне summary —
        // берём фактическое число результатов.
        let hits = sp.results.len();
        self.status_message = Some(format!(
            "Search: {hits} match(es) in {} file(s)",
            summary.files_with_hits
        ));
    }

    /// Замена по файлам (6.5): подтверждение `Window::prompt`, затем
    /// затронутые файлы открываются вкладками (дедуп open_path) и
    /// replace_all применяется В ПАМЯТЬ (dirty = true) — НА ДИСК НЕ
    /// ПИШЕМ (спека + D13); undo доступен в каждой вкладке. Отчёт —
    /// транзиентное сообщение статус-бара.
    pub(crate) fn on_search_replace_in_folder(
        &mut self,
        _e: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sp) = self.search.as_ref() else { return };
        if sp.scanning || sp.query.trim().is_empty() {
            return;
        }
        let Some(Ok(_matcher)) = Self::search_matcher(sp) else { return };
        let paths: Vec<PathBuf> = sp
            .results
            .iter()
            .filter_map(|h| match &h.owner {
                SearchOwner::Path { path, .. } => Some(path.clone()),
                SearchOwner::Tab { .. } => None,
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let tab_paths: Vec<PathBuf> = sp
            .results
            .iter()
            .filter_map(|h| match &h.owner {
                SearchOwner::Tab { index, .. } => self
                    .model
                    .tabs()
                    .get(*index)
                    .and_then(|t| t.path.clone()),
                SearchOwner::Path { .. } => None,
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut all: Vec<PathBuf> = paths;
        all.extend(tab_paths);
        all.sort();
        all.dedup();
        if all.is_empty() {
            return;
        }
        let count = all.len();
        let rx = window.prompt(
            PromptLevel::Info,
            &format!("Replace all matches in {count} file(s)?"),
            Some("Files are opened as modified tabs; nothing is written to disk until you save."),
            &["Replace", "Cancel"],
            cx,
        );
        let paths = all;
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = rx.await else { return };
            if choice != 0 {
                return;
            }
            this.update_in(cx, |ws, _window, cx| {
                ws.apply_folder_replace(&paths, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Применение замен по файлам (после подтверждения): каждое
    /// совпадение пересчитывается по ВКЛАДКЕ (не по диску — повторный
    /// поиск и замена видят изменённое содержимое, D13).
    fn apply_folder_replace(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let Some(sp) = self.search.as_ref() else { return };
        let Some(Ok(matcher)) = Self::search_matcher(sp) else { return };
        let replacement = sp.replacement.clone();
        let mut total = 0usize;
        let mut files_changed = 0usize;
        let mut first_error: Option<String> = None;
        for path in paths {
            match self.model.open_path(path) {
                Ok(ix) => {
                    let Some(tab) = self.model.tab_mut(ix) else { continue };
                    let edits = match matcher.replacements_in_doc(&tab.doc, &replacement) {
                        Ok(e) => e,
                        Err(_) => continue,
                    };
                    let n = tab.apply_search_edits(edits);
                    if n > 0 {
                        total += n;
                        files_changed += 1;
                    }
                }
                Err(e) => {
                    first_error
                        .get_or_insert(format!("Could not open {}: {e}", path.display()));
                }
            }
        }
        self.status_message = Some(if total == 0 {
            "No matches to replace (files may have changed)".to_string()
        } else {
            format!("Replaced {total} occurrence(s) in {files_changed} file(s) — not saved to disk")
        });
        if let Some(e) = first_error {
            self.status_message = Some(e);
        }
        // Совпадения панели устарели — убрать (повторный поиск покажет
        // актуальное состояние вкладок).
        if let Some(sp) = self.search.as_mut() {
            sp.results.clear();
            sp.summary = None;
        }
        cx.notify();
    }
}
