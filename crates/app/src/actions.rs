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
        NewFile, NewFolder, RenameNode, DeleteNode,
        // Split view (8.1/D15.1).
        SplitEditor, ClosePane, ToggleSyncScroll,
        // Темы (8.2/D15.2).
        ThemeSystem, ThemeDark, ThemeLight,
        // Шрифты (8.3/D15.3).
        OpenFontDialog, ZoomIn, ZoomOut, ZoomReset,
        // Язык интерфейса (8.4/D15.4).
        OpenUiLanguageDialog,
        // Раскладка 8.5: вкладки + справочник + язык.
        NewTab, NextTab, PrevTab, ShowShortcuts, CycleLanguage
    ]
);
