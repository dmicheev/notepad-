//! Панель дерева воркспейса (задачи 7.1/7.2): строки дерева в
//! левой панели (клик по файлу — вкладка, клик по папке — раскрыть;
//! правый клик — контекстное меню), overlay-диалоги ввода имён
//! (новый файл/папка/переименование; паттерн goto-диалога D9.2 —
//! ребёнок корня, отдельный фокус), подтверждение удаления
//! (Window::prompt), drag-and-drop перемещения (gpui 0.2.2:
//! on_drag требует ghost-Render view — DragGhost; on_drop на строках
//! папок). Открытие/восстановление воркспейса и watcher (7.3/7.4) —
//! здесь же: open_workspace_folder, поллинг батчей fs_watch.
//!
//! Действия NewFile/NewFolder/Rename/Delete действуют на выделение
//! дерева (нет выделения — корень для создания); вкладки открытых
//! файлов следуют за переименованием/перемещением (retag_path).

use std::path::{Path, PathBuf};

use gpui::{
    App, AppContext, Context, MouseButton, PathPromptOptions, Point, Pixels, PromptLevel, Render,
    Window, div, prelude::*, px, rgb,
};

use crate::actions::{DeleteNode, NewFile, NewFolder, OpenWorkspace, RenameNode};
use crate::fs_tree::FsTree;
use crate::workspace::Workspace;

/// Какой диалог ввода имени открыт.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TreeDialogKind {
    /// Новый файл в каталоге.
    NewFile,
    /// Новая папка в каталоге.
    NewFolder,
    /// Переименование существующего узла.
    Rename { old_path: PathBuf, old_name: String },
}

/// Overlay-диалог ввода имени (7.2).
pub(crate) struct TreeDialog {
    pub kind: TreeDialogKind,
    /// Каталог создания (для New*).
    pub parent: PathBuf,
    pub input: String,
}

/// Контекстное меню дерева (7.2): узел + позиция клика.
#[derive(Clone, Debug)]
pub(crate) struct TreeContextMenu {
    pub node: PathBuf,
    pub is_dir: bool,
    pub position: Point<Pixels>,
}

/// Ghost для drag-and-drop (gpui 0.2.2: on_drag строит Entity<W:
/// Render> — D8.2).
struct DragGhost {
    name: String,
    is_dir: bool,
}

impl Render for DragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .text_size(px(11.0))
            .bg(rgb(crate::theme::current().tabbar))
            .border_1()
            .border_color(rgb(crate::theme::current().accent))
            .rounded_md()
            .text_color(rgb(crate::theme::current().text))
            .child(format!("{} {}", if self.is_dir { "▸" } else { "▪" }, self.name))
    }
}

impl Workspace {
    // ---- Открытие воркспейса (7.1) и watcher (7.3) ----

    /// Cmd+Shift+O: диалог ОС выбора папки (только каталоги).
    pub(crate) fn on_open_workspace(
        &mut self,
        _: &OpenWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Workspace".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(root) = paths.first()
            {
                this.update_in(cx, |ws, _window, cx| ws.open_workspace_folder(root, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Открыть воркспейс: дерево + запуск наблюдателя (старый
    /// заменяется — генерация поллинга растёт, задачи старого
    /// поколения самоостанавливаются).
    pub fn open_workspace_folder(&mut self, root: &Path, cx: &mut Context<Self>) {
        match FsTree::open(root) {
            Ok(tree) => {
                self.tree = Some(tree);
                // Настройки воркспейса (.settings): загрузка под НОВЫЙ
                // корень; гидратация меток вкладок — на следующем кадре.
                self.load_workspace_settings(root);
                self.context_menu = None;
                self.tree_dialog = None;
                self.drop_target = None;
                self.watch_generation += 1;
                match crate::fs_watch::watch_recursive(root) {
                    Ok(watch) => {
                        self.fs_watch = Some(watch);
                        self.spawn_watch_poller(cx);
                    }
                    Err(e) => {
                        self.fs_watch = None;
                        self.set_status_message(crate::i18n::tf(
                            cx,
                            "status.watch_unavailable",
                            &[("err", &e.to_string())],
                        ), cx);
                    }
                }
                cx.notify();
            }
            Err(e) => {
                self.set_status_message(crate::i18n::tf(
                    cx,
                    "status.workspace_open_failed",
                    &[("path", &root.display().to_string()), ("err", &e.to_string())],
                ), cx);
                cx.notify();
            }
        }
    }

    /// Поллинг батчей наблюдателя (150 мс): try_recv без блокировки.
    /// Задача живёт, пока воркспейс того поколения открыт.
    fn spawn_watch_poller(&mut self, cx: &mut Context<Self>) {
        let generation = self.watch_generation;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                let keep = this
                    .update(cx, |ws, cx| {
                        if ws.watch_generation != generation {
                            return false;
                        }
                        let mut touched = false;
                        if let Some(watch) = &ws.fs_watch {
                            while let Ok(_batch) = watch.batches.try_recv() {
                                touched = true;
                            }
                        }
                        if touched {
                            // Полный сброс кэша + чистка исчезнувших
                            // путей (D4 fallback; FsTree::apply_external).
                            if let Some(tree) = &mut ws.tree {
                                tree.apply_external();
                            }
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    // ---- Действия дерева (7.2): горячие клавиши/меню ----

    /// Каталог для создания «в выделении»: выделенная папка, иначе
    /// родитель выделенного файла, иначе корень воркспейса.
    fn creation_parent(&self) -> Option<PathBuf> {
        let tree = self.tree.as_ref()?;
        let selected = tree.selected();
        match selected {
            Some(p) if p.is_dir() => Some(p.to_path_buf()),
            Some(p) => p.parent().map(|d| d.to_path_buf()),
            None => Some(tree.root().to_path_buf()),
        }
    }

    /// Уведомление, когда воркспейс не открыт (действия дерева).
    fn require_tree(&mut self, cx: &mut Context<Self>) -> bool {
        if self.tree.is_none() {
            self.set_status_message(crate::i18n::t(cx, "status.no_workspace"), cx);
            cx.notify();
            false
        } else {
            true
        }
    }

    /// Cmd+N: новый файл в выделении.
    pub(crate) fn on_new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        if !self.require_tree(cx) {
            return;
        }
        if let Some(parent) = self.creation_parent() {
            self.begin_tree_dialog(TreeDialogKind::NewFile, parent, window, cx);
        }
    }

    /// Cmd+Shift+N: новая папка в выделении.
    pub(crate) fn on_new_folder(
        &mut self,
        _: &NewFolder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.require_tree(cx) {
            return;
        }
        if let Some(parent) = self.creation_parent() {
            self.begin_tree_dialog(TreeDialogKind::NewFolder, parent, window, cx);
        }
    }

    /// Shift+F6: переименовать выделенный узел (IntelliJ-жест; F2
    /// занят закладками).
    pub(crate) fn on_rename_node(
        &mut self,
        _: &RenameNode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tree) = self.tree.as_ref() else {
            self.require_tree(cx);
            return;
        };
        let Some(node) = tree.selected().map(|p| p.to_path_buf()) else {
            self.set_status_message(crate::i18n::t(cx, "status.select_to_rename"), cx);
            cx.notify();
            return;
        };
        if node == tree.root() {
            self.set_status_message(crate::i18n::t(cx, "status.cannot_rename_root"), cx);
            cx.notify();
            return;
        }
        let old_name = node
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let parent = node.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        self.begin_tree_dialog(TreeDialogKind::Rename { old_path: node, old_name }, parent, window, cx);
    }

    /// Cmd+Backspace: удалить выделенный узел (с подтверждением).
    pub(crate) fn on_delete_node(
        &mut self,
        _: &DeleteNode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tree) = self.tree.as_ref() else {
            self.require_tree(cx);
            return;
        };
        let Some(node) = tree.selected().map(|p| p.to_path_buf()) else {
            self.set_status_message(crate::i18n::t(cx, "status.select_to_delete"), cx);
            cx.notify();
            return;
        };
        if node == tree.root() {
            self.set_status_message(crate::i18n::t(cx, "status.cannot_delete_root"), cx);
            cx.notify();
            return;
        }
        self.confirm_delete_node(node, window, cx);
    }

    /// Модальное подтверждение удаления (папка — рекурсивно).
    fn confirm_delete_node(&mut self, node: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let is_dir = node.is_dir();
        let name = node
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| node.display().to_string());
        let rx = window.prompt(
            PromptLevel::Warning,
            &crate::i18n::tf(cx, "dialog.delete_title", &[("name", &name)]),
            Some(
                if is_dir {
                    crate::i18n::t(cx, "dialog.delete_folder_note")
                } else {
                    crate::i18n::t(cx, "dialog.delete_file_note")
                }
                .as_str(),
            ),
            &[
                crate::i18n::t(cx, "dialog.delete").as_str(),
                crate::i18n::t(cx, "dialog.cancel").as_str(),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = rx.await else { return };
            if choice == 0 {
                this.update_in(cx, |ws, _window, cx| {
                    if let Some(tree) = &mut ws.tree {
                        match tree.delete(&node) {
                            Ok(()) => {
                                ws.set_status_message(crate::i18n::tf(
                                    cx,
                                    "status.deleted",
                                    &[("name", &name)],
                                ), cx);
                            }
                            Err(e) => {
                                ws.set_status_message(crate::i18n::tf(
                                    cx,
                                    "status.delete_failed",
                                    &[("err", &e.to_string())],
                                ), cx);
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    // ---- Диалог ввода имени ----

    fn begin_tree_dialog(
        &mut self,
        kind: TreeDialogKind,
        parent: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = None;
        self.tree_dialog = Some(TreeDialog { kind, parent, input: String::new() });
        window.focus(&self.tree_focus);
        cx.notify();
    }

    /// Клавиши диалога имён: разбор буфера — overlay_input (без '/' и
    /// контролей, лимит 255), Enter — применить, Escape — отмена.
    pub(crate) fn handle_tree_dialog_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let outcome = self
            .tree_dialog
            .as_mut()
            .map(|dialog| {
                crate::overlay_input::feed(&mut dialog.input, event, 255, |c| c != '/')
            });
        match outcome {
            Some(crate::overlay_input::InputKey::Submit) => {
                let dialog = self.tree_dialog.take().expect("checked above");
                window.focus(&self.active_focus());
                self.apply_tree_dialog(dialog, cx);
            }
            Some(crate::overlay_input::InputKey::Cancel) => {
                self.tree_dialog = None;
                window.focus(&self.active_focus());
            }
            _ => {}
        }
        cx.notify();
    }

    /// Применить диалог: создание/переименование в FsTree; вкладки
    /// следуют за переименованием (retag_path, 7.2).
    fn apply_tree_dialog(&mut self, dialog: TreeDialog, cx: &mut Context<Self>) {
        let name = dialog.input.trim().to_string();
        let Some(tree) = &mut self.tree else { return };
        let result = match &dialog.kind {
            TreeDialogKind::NewFile => tree
                .create_file(&dialog.parent, &name)
                .map(|p| (p, crate::i18n::tf(cx, "status.created_file", &[("name", &name)]))),
            TreeDialogKind::NewFolder => tree
                .create_dir(&dialog.parent, &name)
                .map(|p| (p, crate::i18n::tf(cx, "status.created_folder", &[("name", &name)]))),
            TreeDialogKind::Rename { old_path, .. } => tree
                .rename(old_path, &name)
                .map(|p| (p, crate::i18n::tf(cx, "status.renamed", &[("name", &name)]))),
        };
        match result {
            Ok((path, verb)) => {
                if let TreeDialogKind::Rename { old_path, .. } = &dialog.kind {
                    self.retag_path_all(old_path, &path);
                }
                self.set_status_message(verb, cx);
                // Созданное/переименованное — выделено в дереве.
                if let Some(tree) = &mut self.tree {
                    tree.select(Some(&path));
                }
            }
            Err(e) => {
                self.set_status_message(e.to_string(), cx);
            }
        }
        cx.notify();
    }

    // ---- Контекстное меню (7.2) ----

    /// Открыть контекстное меню на узле (правый клик).
    pub(crate) fn open_context_menu(
        &mut self,
        node: PathBuf,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let is_dir = node.is_dir();
        if let Some(tree) = &mut self.tree {
            tree.select(Some(&node));
        }
        self.context_menu = Some(TreeContextMenu { node, is_dir, position });
        cx.notify();
    }

    /// Закрыть контекстное меню (клик по подложке/действие).
    pub(crate) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    // ---- Drag-and-drop (7.2) ----

    /// Перетащили узел на папку: перемещение + вкладки следуют за
    /// файлом (сценарий спеки «Перемещение файла в дереве»).
    pub(crate) fn tree_move(&mut self, from: &Path, to_dir: &Path, cx: &mut Context<Self>) {
        let Some(tree) = &mut self.tree else { return };
        match tree.move_into(from, to_dir) {
            Ok(new_path) => {
                self.retag_path_all(from, &new_path);
                let name = from
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let dir_name = to_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| to_dir.display().to_string());
                self.set_status_message(crate::i18n::tf(
                    cx,
                    "status.moved",
                    &[("name", &name), ("dir", &dir_name)],
                ), cx);
            }
            Err(e) => {
                self.status_message =
                    Some(crate::i18n::tf(cx, "status.move_failed", &[("err", &e.to_string())]));
            }
        }
        self.drop_target = None;
        cx.notify();
    }

    // ---- Рендер ----

    /// Секция дерева в панели «Проект»: заголовок-воркспейс + строки.
    pub(crate) fn render_tree_section(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let Some(tree) = &mut self.tree else {
            return div()
                .px_3()
                .py_1()
                .text_size(px(11.0))
                .text_color(rgb(crate::theme::current().dim))
                .child(crate::i18n::t(cx, "tree.empty"))
                .child(
                    div()
                        .text_color(rgb(crate::theme::current().panel_header))
                        .child(crate::i18n::t(cx, "tree.hint")),
                );
        };
        let root = tree.root().to_path_buf();
        let root_key = root.clone();
        let root_name = tree.root_name();
        let rows = tree.visible_rows();
        let selected = tree.selected().map(|p| p.to_path_buf());
        let mut section = div().min_h_0();
        // Заголовок воркспейса (приём drop на корень).
        let root_header = div()
            .id("workspace-root-row")
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .text_size(px(11.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(rgb(crate::theme::current().panel_header))
            .bg(rgb(if self.drop_target.as_deref() == Some(root.as_path()) {
                crate::theme::current().selection
            } else {
                crate::theme::current().panel
            }))
            .rounded_sm()
            .debug_selector(move || format!("tree-row:{}", root_key.display()))
            .on_click(cx.listener({
                let root = root.clone();
                move |this, _e, _w, cx| {
                    if let Some(tree) = &mut this.tree {
                        tree.toggle(&root);
                    }
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener({
                    let root = root.clone();
                    move |this, e: &gpui::MouseDownEvent, _w, cx| {
                        this.open_context_menu(root.clone(), e.position, cx);
                    }
                }),
            )
            .on_drop::<PathBuf>(cx.listener({
                let root = root.clone();
                move |this, dragged: &PathBuf, _w, cx| {
                    this.tree_move(dragged, &root, cx);
                }
            }))
            .on_drag_move::<PathBuf>(cx.listener({
                let root = root.clone();
                move |this, e: &gpui::DragMoveEvent<PathBuf>, _w, cx| {
                    let inside = e.bounds.contains(&e.event.position);
                    if inside {
                        if this.drop_target.as_deref() != Some(root.as_path()) {
                            this.drop_target = Some(root.clone());
                            cx.notify();
                        }
                    } else if this.drop_target.as_deref() == Some(root.as_path()) {
                        this.drop_target = None;
                        cx.notify();
                    }
                }
            }))
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(rgb(crate::theme::current().panel_header))
                    .child("▾"),
            )
            .child(div().min_w_0().text_ellipsis().child(root_name));
        section = section.child(root_header);

        for (ix, row) in rows.iter().enumerate() {
            let is_selected = selected.as_deref() == Some(row.path.as_path());
            let is_drop_target =
                row.is_dir && self.drop_target.as_deref() == Some(row.path.as_path());
            let indent = px(10.0 + row.depth as f32 * 12.0);
            let caret = if row.expanded { "▾" } else { "▸" };
            let mut row_div = div()
                .id(("tree-row", ix))
                .flex()
                .items_center()
                .gap_1()
                .pl(indent)
                .pr_2()
                .py_1()
                .text_size(px(11.0))
                .text_color(rgb(if row.is_dir {
                    crate::theme::current().panel_header
                } else {
                    crate::theme::current().text
                }))
                .bg(rgb(if is_drop_target {
                    crate::theme::current().selection
                } else if is_selected {
                    crate::theme::current().close_hover
                } else {
                    crate::theme::current().panel
                }))
                .rounded_sm()
                .hover(|d| d.bg(rgb(crate::theme::current().close_hover)))
                .debug_selector({
                    let key = format!("tree-row:{}", row.path.display());
                    move || key.clone()
                })
                .child(
                    div()
                        .flex_shrink_0()
                        .w(px(12.0))
                        .text_color(rgb(if row.is_dir { crate::theme::current().panel_header } else { crate::theme::current().dim }))
                        .child(if row.is_dir { caret } else { "▪" }),
                )
                .child(div().min_w_0().text_ellipsis().child(row.name.clone()));

            if row.is_dir {
                row_div = row_div
                    .on_click(cx.listener({
                        let path = row.path.clone();
                        move |this, _e, _w, cx| {
                            if let Some(tree) = &mut this.tree {
                                tree.select(Some(&path));
                                tree.toggle(&path);
                            }
                            cx.notify();
                        }
                    }))
                    .on_drop::<PathBuf>(cx.listener({
                        let path = row.path.clone();
                        move |this, dragged: &PathBuf, _w, cx| {
                            this.tree_move(dragged, &path, cx);
                        }
                    }))
                    .on_drag_move::<PathBuf>(cx.listener({
                        let path = row.path.clone();
                        move |this, e: &gpui::DragMoveEvent<PathBuf>, _w, cx| {
                            let inside = e.bounds.contains(&e.event.position);
                            if inside {
                                if this.drop_target.as_deref() != Some(path.as_path()) {
                                    this.drop_target = Some(path.clone());
                                    cx.notify();
                                }
                            } else if this.drop_target.as_deref() == Some(path.as_path()) {
                                this.drop_target = None;
                                cx.notify();
                            }
                        }
                    }));
            } else {
                row_div = row_div.on_click(cx.listener({
                    let path = row.path.clone();
                    move |this, _e, window, cx| {
                        if let Some(tree) = &mut this.tree {
                            tree.select(Some(&path));
                        }
                        this.open_paths(std::slice::from_ref(&path), window, cx);
                    }
                }));
            }

            // Правый клик — контекстное меню (все узлы).
            row_div = row_div.on_mouse_down(
                MouseButton::Right,
                cx.listener({
                    let path = row.path.clone();
                    move |this, e: &gpui::MouseDownEvent, _w, cx| {
                        this.open_context_menu(path.clone(), e.position, cx);
                    }
                }),
            );
            // Перетаскивание файла/папки (ghost — имя узла).
            row_div = row_div.on_drag(row.path.clone(), {
                let name = row.name.clone();
                let is_dir = row.is_dir;
                move |_value: &PathBuf, _offset, _window, cx: &mut App| {
                    cx.new(|_| DragGhost { name: name.clone(), is_dir })
                }
            });
            section = section.child(row_div);
        }
        section
    }

    /// Overlay контекстного меню + подложка (закрытие кликом мимо).
    pub(crate) fn render_context_menu(&self, cx: &Context<Self>) -> gpui::Div {
        let Some(menu) = &self.context_menu else {
            return div();
        };
        let node = menu.node.clone();
        let is_root = self
            .tree
            .as_ref()
            .is_some_and(|t| t.root() == node);
        let mut menu_div = div()
            .absolute()
            .top(menu.position.y)
            .left(menu.position.x)
            .w(px(170.0))
            .bg(rgb(crate::theme::current().tabbar))
            .border_1()
            .border_color(rgb(crate::theme::current().close_hover))
            .rounded_md()
            .text_size(px(11.0))
            .shadow_md()
            .py_1()
            // Клик внутри меню не долетает до подложки (иначе закрытие
            // съедает действие пункта — mouse_down раньше click).
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
        let mut items: Vec<(String, TreeMenuAction)> = vec![
            (crate::i18n::t(cx, "tree.ctx_new_file"), TreeMenuAction::NewFile),
            (crate::i18n::t(cx, "tree.ctx_new_folder"), TreeMenuAction::NewFolder),
        ];
        if !is_root {
            items.push((crate::i18n::t(cx, "tree.ctx_rename"), TreeMenuAction::Rename));
            items.push((crate::i18n::t(cx, "tree.ctx_delete"), TreeMenuAction::Delete));
        }
        for (ix, (label, action)) in items.into_iter().enumerate() {
            let label_for_sel = label.clone();
            menu_div = menu_div.child(
                div()
                    .id(("ctx-menu-item", ix))
                    .px_3()
                    .py_1()
                    .text_color(rgb(crate::theme::current().text))
                    .hover(|d| d.bg(rgb(crate::theme::current().close_hover)))
                    .debug_selector(move || format!("ctx-menu:{label_for_sel}"))
                    .on_click(cx.listener(move |this, _e, window, cx| {
                        this.run_context_menu_action(action.clone(), window, cx);
                    }))
                    .child(label.clone()),
            );
        }

        // Подложка: закрывает меню кликом/правым кликом мимо пунктов.
        div()
            .absolute()
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _e, _w, cx| this.close_context_menu(cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _e, _w, cx| this.close_context_menu(cx)),
            )
            .child(menu_div)
    }

    /// Пункт контекстного меню → действие (каталог создания — узел
    /// меню или его родитель).
    fn run_context_menu_action(
        &mut self,
        action: TreeMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.context_menu.clone() else { return };
        self.context_menu = None;
        let parent = if menu.is_dir {
            menu.node.clone()
        } else {
            menu.node.parent().map(|p| p.to_path_buf()).unwrap_or_default()
        };
        match action {
            TreeMenuAction::NewFile => {
                self.begin_tree_dialog(TreeDialogKind::NewFile, parent, window, cx)
            }
            TreeMenuAction::NewFolder => {
                self.begin_tree_dialog(TreeDialogKind::NewFolder, parent, window, cx)
            }
            TreeMenuAction::Rename => {
                let old_name = menu
                    .node
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.begin_tree_dialog(
                    TreeDialogKind::Rename { old_path: menu.node, old_name },
                    parent,
                    window,
                    cx,
                )
            }
            TreeMenuAction::Delete => self.confirm_delete_node(menu.node, window, cx),
        }
    }

    /// Overlay-диалог ввода имени (паттерн goto-диалога: ребёнок
    /// корня, отдельный фокус — клавиши не идут в редактор).
    pub(crate) fn render_tree_dialog(&self, cx: &Context<Self>) -> gpui::Div {
        let Some(dialog) = &self.tree_dialog else {
            return div();
        };
        let (title, placeholder) = match &dialog.kind {
            TreeDialogKind::NewFile => {
                (crate::i18n::t(cx, "tree.new_file_title"), crate::i18n::t(cx, "tree.name_placeholder"))
            }
            TreeDialogKind::NewFolder => (
                crate::i18n::t(cx, "tree.new_folder_title"),
                crate::i18n::t(cx, "tree.name_placeholder"),
            ),
            TreeDialogKind::Rename { old_name, .. } => {
                (crate::i18n::t(cx, "tree.rename_title"), old_name.to_string())
            }
        };
        div()
            .absolute()
            // Поверх левой панели (подобрано вручную, паттерн goto).
            .top(px(44.0))
            .left(px(8.0))
            .w(px(204.0))
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .bg(rgb(crate::theme::current().tabbar))
            .border_1()
            .border_color(rgb(crate::theme::current().close_hover))
            .rounded_md()
            .text_size(px(12.0))
            .shadow_md()
            .track_focus(&self.tree_focus)
            .on_key_down(cx.listener(Self::handle_tree_dialog_key))
            .child(div().text_color(rgb(crate::theme::current().dim)).child(title))
            .child(
                div().text_color(rgb(if dialog.input.is_empty() { crate::theme::current().dim } else { crate::theme::current().text }))
                    .child(if dialog.input.is_empty() {
                        placeholder
                    } else {
                        dialog.input.clone()
                    }),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgb(crate::theme::current().dim))
                    .child(crate::i18n::t(cx, "tree.dialog_hint")),
            )
    }
}

/// Действия пунктов контекстного меню (клонятся в слушатели).
#[derive(Clone, Debug)]
enum TreeMenuAction {
    NewFile,
    NewFolder,
    Rename,
    Delete,
}
