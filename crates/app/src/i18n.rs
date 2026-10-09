//! Интеграция i18n (8.4/D15.4): переводы — gpui-глобаль (меню строятся
//! из `App`, view-рендеры читают через `Context`-deref), выбор языка —
//! оверлей (View → Interface Language…) и Cmd+Shift+L (цикл по
//! доступным). Каталог внешних переводов: `<data>/notepad-plus/locales`
//! (`{lang}.toml`); en вшит в бинарник. Смена языка пересобирает меню
//! `cx.set_menus` БЕЗ перезапуска.
//!
//! Спека internationalization: все строки UI — через ключи; новый
//! язык = файл в каталоге + выбор в настройках, без пересборки.

use gpui::{App, prelude::*};

use crate::session;

/// Глобальное состояние переводов (одиночное окно MVP).
pub(crate) struct I18nState(pub i18n::I18n);

impl gpui::Global for I18nState {}

/// Каталог внешних переводов (создаётся при необходимости — пользователь
/// может положить туда свой файл языка).
pub(crate) fn locales_dir() -> Option<std::path::PathBuf> {
    session::app_data_dir().map(|d| d.join("locales"))
}

/// Инициализация глобали при старте (язык из сессии; "en" — дефолт).
pub(crate) fn init(cx: &mut App, language: Option<&str>) {
    let lang = language.filter(|l| !l.is_empty()).unwrap_or("en");
    let i18n = i18n::I18n::load(locales_dir().as_deref(), lang);
    cx.set_global(I18nState(i18n));
}

/// Дефолтные переводы (вшитый en) — для контекстов без глобали
/// (gpui::test не выполняет main; продакшн-путь всегда init).
fn default_i18n() -> &'static i18n::I18n {
    static DEFAULT: std::sync::OnceLock<i18n::I18n> = std::sync::OnceLock::new();
    DEFAULT.get_or_init(|| i18n::I18n::load(None, "en"))
}

/// Строка по ключу (выбранный язык → en → ключ).
pub(crate) fn t(cx: &App, key: &str) -> String {
    cx.try_global::<I18nState>()
        .map(|g| g.0.t(key).to_string())
        .unwrap_or_else(|| default_i18n().t(key).to_string())
}

/// Строка с подстановками (`{name}` → значение).
pub(crate) fn tf(cx: &App, key: &str, args: &[(&str, &str)]) -> String {
    cx.try_global::<I18nState>()
        .map(|g| g.0.tf(key, args))
        .unwrap_or_else(|| default_i18n().tf(key, args))
}

/// Текущий язык интерфейса.
pub(crate) fn language(cx: &App) -> String {
    cx.try_global::<I18nState>()
        .map(|g| g.0.language().to_string())
        .unwrap_or_else(|| "en".to_string())
}

/// Доступные языки: en + внешние файлы каталога.
pub(crate) fn available(_cx: &App) -> Vec<String> {
    i18n::I18n::available(locales_dir().as_deref())
}

/// Сменить язык: перезагрузить переводы, пересобрать меню. Вызывается
/// из оверлея выбора и Cmd+Shift+L (переключение языка).
pub(crate) fn set_language(cx: &mut App, lang: &str) {
    let i18n = i18n::I18n::load(locales_dir().as_deref(), lang);
    cx.set_global(I18nState(i18n));
    cx.set_menus(crate::menus::build(cx));
}

impl crate::workspace::Workspace {
    /// Меню View → Interface Language…: оверлей со списком доступных
    /// языков (en + внешние файлы каталога).
    pub(crate) fn on_open_ui_language_dialog(
        &mut self,
        _: &crate::actions::OpenUiLanguageDialog,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.ui_lang_dialog = true;
        window.focus(&self.ui_lang_focus);
        cx.notify();
    }

    /// Клик по языку: применить (переводы + меню) и закрыть.
    pub(crate) fn apply_ui_language(
        &mut self,
        lang: &str,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.ui_lang_dialog = false;
        set_language(cx, lang);
        self.ui_language = lang.to_string();
        window.focus(&self.active_focus());
        cx.notify();
    }

    /// Esc — закрыть оверлей.
    pub(crate) fn handle_ui_language_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if event.keystroke.key == "escape" {
            self.ui_lang_dialog = false;
            window.focus(&self.active_focus());
            cx.notify();
        }
    }

    /// Оверлей: список языков (текущий выделен), клик — применить.
    pub(crate) fn render_ui_language_overlay(
        &self,
        cx: &gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let current = self.ui_language.clone();
        let mut list = gpui::div().flex().flex_col();
        for (li, lang) in available(cx).into_iter().enumerate() {
            let is_current = lang == current;
            let code = lang.clone();
            let lang_for_click = lang.clone();
            list = list.child(
                gpui::div()
                    .id(("ui-lang-item", li))
                    .debug_selector(move || format!("ui-lang-item:{code}"))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _e, window, cx| {
                            cx.stop_propagation();
                            this.apply_ui_language(&lang_for_click, window, cx);
                        }),
                    )
                    .px_2()
                    .py_1()
                    .text_size(gpui::px(12.0))
                    .text_color(gpui::rgb(if is_current {
                        crate::theme::current().accent
                    } else {
                        crate::theme::current().text
                    }))
                    .hover(|d| d.bg(gpui::rgb(crate::theme::current().close_hover)))
                    .child(lang.clone()),
            );
        }
        let overlay = gpui::div()
            .id("ui-lang-overlay")
            .debug_selector(|| "ui-lang-overlay".to_string())
            .absolute()
            .top(gpui::px(44.0))
            .left(gpui::px(340.0))
            .w(gpui::px(220.0))
            .max_h(gpui::px(240.0))
            .flex()
            .flex_col()
            .pb_2()
            .bg(gpui::rgb(crate::theme::current().panel))
            .border_1()
            .border_color(gpui::rgb(crate::theme::current().close_hover))
            .rounded_md()
            .shadow_md()
            .track_focus(&self.ui_lang_focus)
            .on_key_down(cx.listener(Self::handle_ui_language_key))
            // Клик внутри не доходит до подложки.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(
                gpui::div()
                    .px_3()
                    .py_2()
                    .text_size(gpui::px(12.0))
                    .bg(gpui::rgb(crate::theme::current().tabbar))
                    .text_color(gpui::rgb(crate::theme::current().dim))
                    .child(t(cx, "lang.ui_label")),
            )
            .child(
                list.id("ui-lang-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll(),
            );
        // Подложка: клик мимо диалога закрывает (Esc-синоним).
        gpui::div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _e, window, cx| {
                    this.ui_lang_dialog = false;
                    window.focus(&this.active_focus());
                    cx.notify();
                }),
            )
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(|this, _e, window, cx| {
                    this.ui_lang_dialog = false;
                    window.focus(&this.active_focus());
                    cx.notify();
                }),
            )
            .child(overlay)
            .into_any_element()
    }
}
