//! Действия приложения (gpui `actions!`): единственная точка
//! объявления типов действий; биндинги клавиш — в main.rs, обработчики
//! `on_action` — в editor_actions.rs и goto_dialog.rs.

use gpui::actions;

actions!(
    notepad,
    [
        OpenFile, SaveFile, SaveFileAs, CloseTab, Undo, Redo, Quit, GoToLine, AddNextMatch,
        ToggleBookmark, NextBookmark, PrevBookmark, FoldAll, UnfoldAll,
        SortAsc, SortDesc, SortAscIgnoreCase, SortDescIgnoreCase,
        RemoveDuplicateLines, RemoveEmptyLines,
        UpperCase, LowerCase, TitleCase, SentenceCase
    ]
);
