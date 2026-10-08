# Design

## Context

MVP-приложение (gpui 0.2.2): layout — корневая колонка `Workspace::render` (панель проекта слева, область split-панелей с `tab_bar` + редактор, статус-бар); 36 действий живут в нативном меню (`menus.rs`) и кеймапе (`keymap.rs`: `KeyDef`-таблица с i18n-подписями `shortcuts.action.*`); оверлей-справочник сочетаний уже есть (`Cmd+/`, `render_shortcuts_overlay` в `keymap.rs`, правый верхний угол, без поиска). Asset-ов и иконок нет: `Application::new()` без `with_assets`. Сессия (`session.rs`) уже сохраняет тему/шрифт/вкладки. Change `markdown-preview` спланирован, но не применён.

Требования — в `specs/toolbar/spec.md` и `specs/editor-ui/spec.md` этого change.

## Goals / Non-Goals

**Goals:**
- Тулбар во всю ширину сверху: все ключевые действия в один клик, группы + выпадающие списки для вариантов, тултипы с клавишами, toggle-состояния.
- Монохромные SVG-иконки, красящиеся темой; встраивание без внешних файлов (asset-источник в бинарнике).
- Справочник сочетаний: центрированное модальное окно с поиском; открытие кнопкой «?», `Cmd+/`, Help-меню.
- Переиспользование: кнопки диспатчат те же `Action`-типы, тултипы — из готовых i18n-меток `KEYMAP`.

**Non-Goals:**
- Кастомизация тулбара пользователем (состав/порядок групп, drag) — follow-up.
- Кнопки для всех 36 действий без выпадающих списков (36 кнопок в ряд не помещаются — группы Текст/Тема сознательно выпадающие).
- Иконки в нативных меню (macOS рисует свои) и в дереве проекта.
- Toolbar溢overflow-меню «⋯» (вместо него горизонтальная прокрутка — проще и предсказуемее).

## Decisions

### D1. Размещение и переключатель

Полоса `render_toolbar` первой дочерней в корневой колонке `Workspace::render` (выше `tree_panel` и области панелей): высота ~36 px, фон `theme.tabbar`, разделители групп — `w(px(1))` с `theme.border`. Раскладка ниже не меняется — колонка просто получает/теряет ребёнка.

Переключатель: `ToggleToolbar` action + пункт View (`menu.view.toolbar`); состояние `Workspace::toolbar_visible: bool` (true по умолчанию), persist в `SessionState::toolbar_visible: Option<bool>` по образцу `theme`. Горячая клавиша не назначаем (редкое действие, меню достаточно).

### D2. Иконки: встроенный SVG-набор + AssetSource

- Набор: подмножество Lucide (MIT) ~25 иконок 24×24, `stroke="currentColor"`, `stroke-width=2`, без заливки — редактируем под текущColor при необходимости.
- Хранение: `crates/app/assets/icons/*.svg` + `app/src/icons.rs`: `ICONS: &[(&str, &str)]` через `include_str!` (карта путь→svg) и `struct EmbeddedAssets` — имплементация `gpui::AssetSource` (`load`/`list` по карте). `main.rs`: `Application::new().with_assets(EmbeddedAssets)`. Никаких внешних файлов и новых зависимостей.
- Рендер: `gpui::svg().path("icons/save.svg").size_4().text_color(theme.text)` — gpui красит монохромные SVG цветом текста. Имена иконок соответствуют действиям (`file-plus`, `folder-open`, `save`, `undo`, …, `help`).
- Риск: если раскраска `currentColor` в 0.2.2 не сработает — fallback в той же задаче: прогнать SVG-строки на загрузке, заменив `currentColor` на hex активной темы (иконки перегенерируются при смене темы, кэш `icons.rs`).

### D3. Кнопки, группы, действия, тултипы

`toolbar.rs`: декларативная таблица (по духу `KEYMAP`):

```rust
struct ToolDef { id, icon, action: ToolAction, group, toggle: Option<ToggleSource> }
enum ToolAction { Dispatch(ToolbarAction), Dropdown(DropdownKind) }
```

- Клик = `window.dispatch_action` того же типа, что меню/клавиша (`SaveFile`, `Undo`, `FindInFile`, …) — никакой дублирующей логики.
- Группы по спеке: Файл / Правка / Поиск / Навигация / Вид / «?»; между группами разделители. Кнопка Markdown Preview входит в таблицу, но рендерится/диспатчится только когда действие доступно (см. D6).
- Тултип: hover-состояние кнопки + `gpui::anchored` (в 0.2.2 нет готового Tooltip — ui-крейт Zed; `anchored` в крейте есть). Текст: i18n-метка действия + `pretty_key` из `KEYMAP` (переиспользуем `shortcuts.action.*` — новые ключи только для групп «Текст»/«Тема» и «?»). Тултип без задержки (MVP), исчезает при уходе курсора/клике.
- Кнопка: `div().id(..).w_7().h_7().rounded().hover(bg)` — паттерн кнопок поиска/оверлеев приложения; иконка по центру.

### D4. Выпадающие группы: `anchored` + паттерн контекстного меню

Дропдауны «Текст» (10 операций) и «Тема» (System/Dark/Light) — `gpui::anchored()` от кнопки, фон `theme.panel`, пункты как в меню (`menu.text.*`, `menu.view.theme.*`). Открытый дропдаун — состояние `Workspace::toolbar_dropdown: Option<DropdownKind>` (одновременно не больше одного; повторный клик/выбор/Esc/клик-вне закрывают). Паттерн уже отработан в `tree_panel::TreeContextMenu` — переиспользуем приёмы (клик-вне через подложку, `track_focus`).

«Тема»: активный пункт отмечён галочкой/фоном (`theme_kind`), клик — то же, что пункты меню `ThemeSystem/Dark/Light`.

### D5. Toggle-состояния

`ToggleSource` читает источники истины из `Workspace`: `SyncScroll → ws.sync_scroll`, `MarkdownPreview → ws.markdown_preview.is_some()` (поле из change `markdown-preview`), для «Темы» нажатое состояние не нужно (отмечается пункт дропдауна). Нажатая кнопка: фон `theme.selection`/рамка. Меняются любым путём (меню/клавиша/кнопка) → те же поля → тулбар перекрашивается общим `cx.notify()`.

### D6. Зависимость от `markdown-preview`

Кнопка и toggle предпросмотра объявлены в таблице D3, но рендерятся под флагом: действие `ToggleMarkdownPreview` существует только после применения change `markdown-preview`. Вариант интеграции: в `markdown-preview` change добавляется задача «кнопка тулбара» (см. tasks 6.3 этого change — двусторонняя связка: применяем сначала preview, потом тулбар; либо тулбар с флагом). Выбрано: порядок применения «сначала markdown-preview, потом toolbar-and-shortcuts»; в tasks markdown-preview добавлена задача кнопки (6.3 синхронизирует).

### D7. Окно справочника: центрирование + поиск

Переработка `render_shortcuts_overlay`:
- Центрирование: подложка `absolute().size_full()` + `flex().items_center().justify_center()` — окно фикс. ширины (~560 px, max 70% ширины окна) и `max_h` ~70% высоты; вместо текущих фиксированных top/left.
- Поиск: поле ввода в шапке окна (паттерн фокуса/ввода — `goto_dialog.rs`: `track_focus`, `on_key_down`, Esc-закрытие), состояние `shortcuts_query: String` в `Workspace`. Фильтр: `query.is_empty() || label_i18n.to_lowercase().contains(q) || pretty_key(key).to_lowercase().contains(q)`; секции без совпадений скрываются; пустой результат — сообщение `shortcuts.no_matches`.
- Открытие: существующий `ShowShortcuts` (`Cmd+/`, Help-меню) + кнопка «?» из D3 — один и тот же обработчик `on_show_shortcuts`.

### D8. Переполнение

Контейнер кнопок: `div().flex().overflow_x_scroll()` — горизонтальная прокрутка без переноса; группы не сжимаются (`flex_shrink_0` у кнопок). Высота тулбара фиксирована — layout ниже не дёргается.

## Risks / Trade-offs

- **Раскраска SVG**: `currentColor` в gpui-svg — проверить в задаче 2.1 spike'ом; fallback-механизм описан в D2 (подмена цвета при загрузке), оба пути не добавляют зависимостей.
- **Тултипы на anchored**: `anchored` рассчитан на такие поповеры; если поведёт себя некорректно при скролле тулбара — упрощение: тултип фиксированной позиции над кнопкой через `absolute`-координаты курсора.
- **Производительность**: тулбар — статичная строка ~25 лёгких элементов, рендер каждый кадр пренебрежим (для сравнения: editor рендерит сотни строк).
- **Плотность на минимальной ширине окна**: h-scroll вместо overflow-меню — осознанное упрощение MVP (в Non-Goals).

## Migration Plan

Тулбар включён по умолчанию — видимый новый UI, старые пути (меню/клавиши) не меняются; `SessionState` получает опциональное поле — старые сессии читаются без миграции (`Option<bool>` → default true).

## Open Questions

- Финальный состав иконок (какие именно Lucide-глифы) — при реализации задачи 2.2, критерий: различимость при 16 px (спека это требует).
