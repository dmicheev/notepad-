//! Действия приложения (gpui `actions!`): единственная точка
//! объявления типов действий; биндинги клавиш — в main.rs, обработчики
//! `on_action` — в editor_actions.rs, goto_dialog.rs и search_panel.rs.

use gpui::actions;

actions!(
    notepad,
    [
        OpenFile, OpenWorkspace, SaveFile, SaveFileAs, CloseTab, Undo, Redo, Quit, GoToLine,
        AddNextMatch, ToggleBookmark, NextBookmark, PrevBookmark, FoldAll, UnfoldAll,
        SortAsc, SortDesc, SortAscIgnoreCase, SortDescIgnoreCase,
        RemoveDuplicateLines, RemoveEmptyLines,
        UpperCase, LowerCase, TitleCase, SentenceCase,
        FindInFile, FindInTabs, FindInFolder,
        NewFile, NewFolder, RenameNode, DeleteNode
    ]
);
