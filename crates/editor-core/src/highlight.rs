//! Движок подсветки (задача 5.2, D12.1): инкрементальная раскраска
//! ТОЛЬКО видимых строк через syntect, GPUI-free. Чистые функции
//! документа → цветовые диапазоны; палитра (класс → цвет) — в app
//! (theme.rs).
//!
//! ## parse-state (ключевой дизайн-вопрос, D12.1)
//! Подсветка строки syntect-ом зависит от контекста с начала файла;
//! полный препарс 20 МБ запрещён бюджетом. Решение — ГИБРИД (вариант
//! а+б по задачнику, подтверждён замерами `bench_highlight_components`):
//! - **чекпоинты** `(ParseState, ScopeStack)` каждые `CHECKPOINT_EVERY`
//!   строк разобранной области (прыжок назад/внутри — почти бесплатно);
//! - **стеки на каждую строку окна** (правка инвалидирует от строки
//!   правки; ближайший валидный стек — в пределах окна, переразбор
//!   правки ≈ окно, а не 1024 строки);
//! - **догон с бюджетом времени**: прыжок в неразобранную область
//!   продвигает разбор от ближайшего чекпоинта порциями ≤
//!   `CATCH_UP_BUDGET` на кадр (рендер на кадр — без фризов), окно
//!   подсвечивается прогрессивно по мере догонa.
//!
//! ## Ловушки
//! - набор newlines ждёт строки С терминатором: кормим `line_str` и
//!   дозаправляем `\n` на хвостовой строке (иначе хвост без подсветки);
//! - патологические строки (minified JS — мегабайт в одной строке):
//!   `LINE_CHAR_CAP` — выше капа строка без подсветки И не кормится
//!   парсеру (стек проходит насквозь; состояние после такой строки
//!   может быть неточным — осознанный компромисс, D12.3);
//! - кэш живёт в `BufferTab` (вне процессора uniform_list), НЕ
//!   переаллоцируется на кадр; каретки/выделения кэш не трогают.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use syntect::parsing::{ParseState, ScopeStack, ScopeStackOp, SyntaxReference};

use crate::document::Document;
use crate::syntax::syntax_set;

/// Кап длины строки (в char) для подсветки: длиннее — без подсветки
/// и без кормления парсеру (D12.3; значение — по замерам: 8 КБ
/// парсится ~доли мс, 1 МБ строка — десятки мс, т.е. убивает кадр).
pub const LINE_CHAR_CAP: usize = 8192;

/// Бюджет догоняющего разбора на один вызов `ensure_window` (кадр):
/// держит рендер в <8 мс даже при прыжке в конец 20 МБ файла.
pub const CATCH_UP_BUDGET: Duration = Duration::from_millis(2);

/// Шаг чекпоинтов стека (строк).
/// Шаг чекпоинтов (строк): фон/догон сохраняют состояние разбора
/// каждые CHECKPOINT_EVERY границ — прыжок рядом с чекпоинтом почти
/// бесплатен. `pub` — app фильтрует мелкие файлы фонового прохода.
pub const CHECKPOINT_EVERY: usize = 1024;

/// Потолок кэша спанов; при превышении чистим вне окна ± запас.
const SPANS_LIMIT: usize = 16384;

/// Класс стиля (маппинг на палитру — app/theme.rs; 0 = обычный текст).
pub type StyleClass = u8;
pub const CLASS_COMMENT: StyleClass = 1;
pub const CLASS_STRING: StyleClass = 2;
pub const CLASS_KEYWORD: StyleClass = 3;
pub const CLASS_NUMBER: StyleClass = 4;
pub const CLASS_FUNCTION: StyleClass = 5;
pub const CLASS_TYPE: StyleClass = 6;
pub const CLASS_CONSTANT: StyleClass = 7;
/// CSV/TSV (D12.4): колонки чередуются цветами — 4 класса с ротацией
/// по индексу колонки (соседние колонки всегда различимы).
pub const CLASS_COLUMN_A: StyleClass = 8;
pub const CLASS_COLUMN_B: StyleClass = 9;
pub const CLASS_COLUMN_C: StyleClass = 10;
pub const CLASS_COLUMN_D: StyleClass = 11;

/// Класс колонки №`col` (ротация 4 цветов).
fn column_class(col: usize) -> StyleClass {
    match col % 4 {
        0 => CLASS_COLUMN_A,
        1 => CLASS_COLUMN_B,
        2 => CLASS_COLUMN_C,
        _ => CLASS_COLUMN_D,
    }
}

/// Цветовой диапазон внутри строки: char-координаты от начала строки
/// (без терминатора), `class` — класс стиля.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub class: StyleClass,
}

/// Состояние разбора на границе строки `line` (после строк `< line`).
/// Обе половины обязательны: `ParseState` — контекстный стек grammar,
/// `ScopeStack` — скоупы для цвета (HighlightLines держит обе, D12.1).
/// `pub` для фоновой порции (BackgroundChunk); поля приватны.
#[derive(Debug, Clone)]
pub struct LineState {
    parse: ParseState,
    scopes: ScopeStack,
}

/// Порция фонового прохода раскраски (см. [`BackgroundWalk`]).
#[derive(Debug)]
pub struct BackgroundChunk {
    /// Спаны разобранных строк порции (старые координаты документа —
    /// снимок, на котором ходка стартовала).
    pub spans: Vec<(usize, Arc<Vec<Span>>)>,
    /// Чекпоинты (граница строки → состояние), кратно
    /// `CHECKPOINT_EVERY`.
    pub checkpoints: Vec<(usize, LineState)>,
    /// Документ пройден до конца.
    pub done: bool,
}

/// Фоновый проход раскраски всего документа чекпоинтами: прыжки вглубь
/// большого файла не должны ждать долгого кадрового догонa — ходка
/// строит чекпоинты сразу (любой прыжок = ≤ CHECKPOINT_EVERY строк
/// догонa). Владеет собственным состоянием, работает на КОПИИ rope в
/// фоновой задаче; порции сливаются в движок потребителем через
/// [`HighlightEngine::merge_background`] (валидность — по первой
/// строке правок с момента старта; смена языка — сверкой имени на
/// стороне потребителя).
pub struct BackgroundWalk {
    state: LineState,
    line: usize,
}

impl BackgroundWalk {
    /// Начать от самого дальнего чекпоинта движка (прогресс рендера
    /// переиспользуется). `None` — синтаксиса нет (Plain Text/порог)
    /// или CSV (раскраска дешёвая, фон не нужен).
    pub fn start(engine: &HighlightEngine) -> Option<Self> {
        let syntax = engine.syntax?;
        if engine.csv {
            return None;
        }
        let (line, state) = match engine.states.last_key_value() {
            Some((l, s)) => (*l, s.clone()),
            None => (0, LineState { parse: ParseState::new(syntax), scopes: ScopeStack::new() }),
        };
        Some(BackgroundWalk { state, line })
    }

    /// Курсор ходки: следующая строка разбора (граница валидности
    /// `bg_valid_upto`: правка выше курсора ходку перезапускают).
    pub fn line(&self) -> usize {
        self.line
    }

    /// Разобрать очередную порцию строк в рамках `budget` (не меньше
    /// одной строки за вызов — сходимость потребителя гарантирована):
    /// спаны порции + чекпоинты каждых CHECKPOINT_EVERY границ.
    /// Разбор идентичен ensure_window (parse_line + build_spans; кап
    /// длины — пустые спаны).
    pub fn chunk(&mut self, doc: &Document, budget: Duration) -> BackgroundChunk {
        let total = doc.len_lines();
        let deadline = Instant::now() + budget;
        let mut out =
            BackgroundChunk { spans: Vec::new(), checkpoints: Vec::new(), done: false };
        while self.line < total {
            let mut text = doc.line_str(self.line); // с терминатором (кроме хвоста)
            if !text.ends_with('\n') {
                text.push('\n'); // ловушка newlines-набора
            }
            let spans = if text.chars().count() > LINE_CHAR_CAP + 1 {
                Vec::new() // кап длины: без цвета, стек проходит насквозь
            } else {
                let ops = self.state.parse.parse_line(&text, syntax_set());
                let empty = Vec::new();
                let ops = ops.as_ref().unwrap_or(&empty);
                build_spans(&text, ops, &mut self.state.scopes)
            };
            out.spans.push((self.line, Arc::new(spans)));
            self.line += 1;
            if self.line.is_multiple_of(CHECKPOINT_EVERY) {
                out.checkpoints.push((self.line, self.state.clone()));
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        out.done = self.line >= total;
        out
    }
}

/// Движок подсветки одного документа (владелец — `BufferTab`).
#[derive(Debug)]
pub struct HighlightEngine {
    /// Активный синтаксис; `None` — подсветка выключена (языка нет,
    /// порог 20 МБ, ручной Plain Text).
    syntax: Option<&'static SyntaxReference>,
    /// CSV/TSV-режим (D12.4): спаны строит `csv_spans` (колонки
    /// чередуются цветами), syntect-парсер не гоняется.
    csv: bool,
    /// Кэш спанов: строка → цветовые диапазоны (декорированные строки).
    spans: HashMap<usize, Arc<Vec<Span>>>,
    /// Чекпоинты + стеки окна: граница строки → состояние разбора.
    states: BTreeMap<usize, LineState>,
    /// Бюджет догонa (по умолчанию `CATCH_UP_BUDGET`; тесты ставят
    /// большой — детерминизм не зависит от нагрузки CPU).
    budget: Duration,
}

impl Default for HighlightEngine {
    fn default() -> Self {
        HighlightEngine {
            syntax: None,
            csv: false,
            spans: HashMap::new(),
            states: BTreeMap::new(),
            budget: CATCH_UP_BUDGET,
        }
    }
}

impl HighlightEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Бюджет догоняющего разбора на вызов (кадр).
    pub fn set_budget(&mut self, budget: Duration) {
        self.budget = budget;
    }

    /// Активный синтаксис (имя — для статус-бара/тестов).
    pub fn syntax_name(&self) -> Option<&str> {
        self.syntax.map(|s| s.name.as_str())
    }

    /// Сменить синтаксис (`None` — выключить). Смена имени инвалидирует
    /// весь кэш; тот же синтаксис — no-op (прогрев не сбрасывает кэш).
    pub fn set_syntax(&mut self, syntax: Option<&'static SyntaxReference>) {
        let new_name = syntax.map(|s| s.name.as_str());
        if new_name == self.syntax_name() {
            return;
        }
        self.syntax = syntax;
        self.csv = syntax.is_some_and(|s| s.name == "CSV");
        self.reset_cache();
    }

    /// Полный сброс кэшей (undo/redo: неизвестны строки правок —
    /// переразбор ленивый, чекпоинты построятся заново; D12.1).
    pub fn reset_cache(&mut self) {
        self.spans.clear();
        self.states.clear();
    }

    /// Инвалидация при правке на строке `line` (до-координаты): строки
    /// `>= line` менялись (спаны), стеки границ `> line` недействительны;
    /// каретки/выделения сюда не попадают (apply_tx только).
    pub fn invalidate_from(&mut self, line: usize) {
        self.spans.retain(|&l, _| l < line);
        let stale = self.states.split_off(&(line + 1));
        drop(stale);
    }

    /// Спаны строки (после `ensure_window`); `None` — ещё не догнали/
    /// выключено, пустой Vec — кап длины (рендер одинаков: без цвета).
    pub fn spans(&self, line: usize) -> Option<&Arc<Vec<Span>>> {
        self.spans.get(&line)
    }

    /// Прогреть подсветку окна `[first, first+count)` (видимые строки,
    /// real-координаты). Догон к `first` и разбор окна — в рамках
    /// `CATCH_UP_BUDGET`; не догнали — окно без подсветки, продолжим со
    /// следующего кадра (прогрессивная подсветка, без фризов).
    ///
    /// Возвращает `true` — окно покрыто спанами ЦЕЛИКОМ (вызов
    /// догнал), `false` — бюджет исчерпан, есть хвост работы: рендер
    /// обязан запросить следующий кадр (иначе после резкого прыжка
    /// текст без цвета до первого события мыши — баг ручной проверки:
    /// плавный скролл кадры даёт, прыжок — один кадр). Дедлайн
    /// проверяется ПОСЛЕ разбора строки: каждый вызов разбирает хотя
    /// бы одну строку — прогресс гарантирован, notify-цикл рендера
    /// сходится при любом бюджете.
    pub fn ensure_window(&mut self, doc: &Document, first: usize, count: usize) -> bool {
        let Some(_syntax) = self.syntax else { return true };
        let total = doc.len_lines();
        let first = first.min(total.saturating_sub(1));
        let last = (first + count).min(total); // [first, last)

        // Догон: ближайший стек <= first, разбираем от него. Дедлайн
        // ставится ПОСЛЕ инициализации состояния: первый
        // ParseState::new синтаксиса лениво десериализует контексты
        // (OnceCell в SyntaxReference) — разовая цена, не бюджет кадра
        // (прогрев app платит её вне рендера, D12.1).
        let start = self.states.range(..=first).next_back().map(|(l, _)| *l);
        let mut state = match start {
            Some(l) => self.states.get(&l).expect("range entry").clone(),
            None => LineState { parse: ParseState::new(_syntax), scopes: ScopeStack::new() },
        };
        let deadline = Instant::now() + self.budget;
        // Догон и окно собирают спаны ОДИНАКОВО (parse_into всегда
        // collect): иначе строки зоны догонa без спанов перепарсиваются
        // каждым кадром (тёплый скролл деградировал до 2 мс/кадр —
        // поймано бенчмарком warm window). Кэш растёт, но чистится
        // (prune) по SPANS_LIMIT.
        let mut line = start.unwrap_or(0);
        while line < first {
            // Уже разобранная строка — пропуск (map-lookup'ы только).
            if self.cached(&line) {
                if let Some(next) = self.states.get(&(line + 1)) {
                    state = next.clone();
                }
                line += 1;
                continue;
            }
            self.parse_into(doc, line, &mut state);
            line += 1;
            if line.is_multiple_of(CHECKPOINT_EVERY) {
                self.states.insert(line, state.clone());
            }
            if Instant::now() >= deadline {
                // Прогресс сохраняем: чекпоинт на текущей границе.
                self.states.insert(line, state);
                self.prune_spans(first, last);
                return false;
            }
        }

        // Окно: спаны + стек на каждую строку (стеки окна — быстрая
        // инвалидация правок, D12.1). Дедлайн — ПОСЛЕ разбора: вызов
        // без прогресса зависил бы notify-цикл рендера (нулевой
        // бюджет + пустое окно).
        while line < last {
            if self.cached(&line) {
                if let Some(next) = self.states.get(&(line + 1)) {
                    state = next.clone();
                }
                line += 1;
                continue;
            }
            self.parse_into(doc, line, &mut state);
            line += 1;
            self.states.insert(line, state.clone());
            if Instant::now() >= deadline {
                break;
            }
        }
        if line > first {
            self.states.insert(line, state);
        }

        // Чистка: чекпоинты + окно (+1) оставляем, прочие — вон.
        // Прогресс-точка догонa (максимум <= first) тоже живёт.
        let restart = self.states.range(..=first).next_back().map(|(l, _)| *l);
        self.states.retain(|&l, _| {
            l % CHECKPOINT_EVERY == 0 || (first <= l && l <= last + 1) || Some(l) == restart
        });
        self.prune_spans(first, last);
        // Окно покрыто? (пустой Vec — тоже покрыт: кап длины строки)
        (first..last).all(|l| self.spans.contains_key(&l))
    }

    /// Строка в кэше (спаны есть) — рендер и повторный проход не
    /// требуют разбора.
    fn cached(&self, line: &usize) -> bool {
        self.spans.contains_key(line)
    }

    /// Влить фоновую порцию ([`BackgroundWalk`]). `valid_upto` —
    /// первая строка правок с момента старта ходки: чекпоинт на
    /// границе L валиден при `L <= valid_upto` (строки < L не
    /// менялись); спаны строк `>= valid_upto` устарели — кадровый
    /// ensure_window пересчитает. Чекпоинты вливаются всегда (их
    /// ~len_lines/1024 — память ничтожна), спаны — под SPANS_LIMIT.
    /// Видимые строки порции немедленно красят рендер (notify
    /// делает потребитель).
    pub fn merge_background(&mut self, chunk: BackgroundChunk, valid_upto: usize) {
        for (line, spans) in chunk.spans {
            if line < valid_upto && self.spans.len() < SPANS_LIMIT {
                self.spans.insert(line, spans);
            }
        }
        for (line, state) in chunk.checkpoints {
            if line <= valid_upto {
                self.states.insert(line, state);
            }
        }
    }

    /// Чекпоинт-границы движка (тесты фонового прохода/прыжков).
    pub fn checkpoint_lines(&self) -> Vec<usize> {
        self.states.keys().copied().collect()
    }

    /// Почистить кэш спанов вне окна ± запас (потолок SPANS_LIMIT —
    /// защита памяти на длинных догонax по 20 МБ файлу).
    fn prune_spans(&mut self, first: usize, last: usize) {
        if self.spans.len() > SPANS_LIMIT {
            let lo = first.saturating_sub(4096);
            let hi = last + 4096;
            self.spans.retain(|&l, _| lo <= l && l <= hi);
        }
    }

    /// Разобрать строку `line`: обновить `state`, положить спаны.
    /// CSV-режим (D12.4): спаны колонок, состояние проходит насквозь.
    /// Кап длины: строка не кормится, спаны пустые, стек проходит
    /// насквозь (D12.3).
    fn parse_into(&mut self, doc: &Document, line: usize, state: &mut LineState) {
        let mut text = doc.line_str(line); // с терминатором (кроме хвоста)
        if !text.ends_with('\n') {
            text.push('\n'); // ловушка newlines-набора: хвостовая строка
        }
        let text_chars = text.chars().count();
        if text_chars > LINE_CHAR_CAP + 1 {
            self.spans.insert(line, Arc::new(Vec::new()));
            return;
        }
        let spans = if self.csv {
            csv_spans(&text)
        } else {
            let ops = state.parse.parse_line(&text, syntax_set());
            let empty = Vec::new();
            let ops = ops.as_ref().unwrap_or(&empty);
            build_spans(&text, ops, &mut state.scopes)
        };
        self.spans.insert(line, Arc::new(spans));
    }

    /// Разобрать строку наружу (тесты): спаны + состояние после строки.
    #[cfg(test)]
    fn parse_line_standalone(
        syntax: &'static SyntaxReference,
        text: &str,
        prev: Option<&LineState>,
    ) -> (Vec<Span>, LineState) {
        let mut t = text.to_string();
        if !t.ends_with('\n') {
            t.push('\n');
        }
        let mut state = match prev {
            Some(s) => s.clone(),
            None => LineState { parse: ParseState::new(syntax), scopes: ScopeStack::new() },
        };
        let ops = state.parse.parse_line(&t, syntax_set()).expect("parse");
        let spans = build_spans(&t, &ops, &mut state.scopes);
        (spans, state)
    }
}

/// Спаны строки из ops parse_line (байтовые offset → char). Текст
/// `[prev_byte..op_offset)` красится стеком ПОСЛЕ применения всех ops
/// с offset `<= prev_byte` (семантика HighlightIterator).
fn build_spans(text: &str, ops: &[(usize, ScopeStackOp)], scopes: &mut ScopeStack) -> Vec<Span> {
    // char-индексы строки БЕЗ терминатора (его не красим).
    let content_chars = text.chars().count().saturating_sub(1);
    let char_of: Vec<usize> = text
        .char_indices()
        .map(|(b, _)| b)
        .chain(std::iter::once(text.len()))
        .collect(); // char idx → byte idx
    let mut spans: Vec<Span> = Vec::new();
    let mut push = |start_char: usize, end_char: usize, class: StyleClass| {
        if end_char <= start_char || class == 0 {
            return;
        }
        let end_char = end_char.min(content_chars);
        let start_char = start_char.min(end_char);
        if end_char > start_char {
            spans.push(Span {
                start: start_char as u32,
                end: end_char as u32,
                class,
            });
        }
    };
    let mut cur = class_for_scopes(scopes);
    let mut prev_char = 0usize;
    let mut prev_byte = 0usize;
    for &(offset, ref op) in ops {
        // offset — байтовая позиция применения op; текст до неё —
        // предыдущим стеком.
        if offset > prev_byte {
            let end_char = char_pos(&char_of, offset);
            push(prev_char, end_char, cur);
            prev_char = end_char;
            prev_byte = offset;
        }
        let _ = scopes.apply(op); // pop пустого стека — ошибка, игнорируем
        cur = class_for_scopes(scopes);
    }
    push(prev_char, content_chars, cur);
    // Слить соседние диапазоны одного класса (меньше сегментов рендера).
    let mut merged: Vec<Span> = Vec::with_capacity(spans.len());
    for s in spans {
        match merged.last_mut() {
            Some(last) if last.class == s.class && last.end == s.start => last.end = s.end,
            _ => merged.push(s),
        }
    }
    merged
}

/// Байтовая позиция → char-позиция (char_of — байтовые индексы по
/// char-порядку; бинарный поиск).
fn char_pos(char_of: &[usize], byte: usize) -> usize {
    match char_of.binary_search(&byte) {
        Ok(i) => i,
        Err(i) => i, // внутри multibyte-символа — граница не бывает
    }
}

/// Цветовые диапазоны CSV/TSV-строки (D12.4): колонки чередуются 4
/// цветами (ротация по индексу). Разделитель — большинством в строке
/// (`,` для CSV, `\t` для TSV; ничья → запятая), кавычки RFC-4180:
/// запятая/таб внутри `"…"` не разделяют, `""` — экранированная
/// кавычка. Пустые поля не красятся, но индекс колонки считают.
/// Координаты — char, без терминатора. Подсветка построчная
/// (stateless) — состояние разбора не меняется.
fn csv_spans(text: &str) -> Vec<Span> {
    let content = text.strip_suffix('\n').unwrap_or(text);
    if content.is_empty() {
        return Vec::new();
    }
    let commas = content.matches(',').count();
    let tabs = content.matches('\t').count();
    let sep = if tabs > commas { '\t' } else { ',' };
    let mut spans: Vec<Span> = Vec::new();
    let mut col = 0usize;
    let mut field_start = 0usize; // char-позиция начала поля
    let mut char_idx = 0usize;
    let mut in_quotes = false;
    let mut chars = content.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if !in_quotes => in_quotes = true,
            '"' if chars.peek() == Some(&'"') => {
                chars.next(); // экранированная ""
                char_idx += 1;
            }
            '"' => in_quotes = false,
            c if c == sep && !in_quotes => {
                push_column(&mut spans, field_start, char_idx, col);
                col += 1;
                field_start = char_idx + 1;
            }
            _ => {}
        }
        char_idx += 1;
    }
    push_column(&mut spans, field_start, char_idx, col);
    spans
}

/// Добавить спан поля (пустые поля пропускаются — но чередование
/// цветов сохраняется: класс по индексу колонки).
fn push_column(spans: &mut Vec<Span>, start: usize, end: usize, col: usize) {
    if end <= start {
        return;
    }
    spans.push(Span {
        start: start as u32,
        end: end as u32,
        class: column_class(col),
    });
}

/// Класс стиля по стеку скоупов: первый матч от ВНУТРЕННЕГО скоупа.
fn class_for_scopes(stack: &ScopeStack) -> StyleClass {
    for scope in stack.scopes.iter().rev() {
        let s = scope.build_string();
        // Порядок mattered: частные случаи раньше общих.
        if s.starts_with("punctuation.definition.comment") {
            return CLASS_COMMENT;
        }
        if s.starts_with("comment") {
            return CLASS_COMMENT;
        }
        if s.starts_with("string") || s.starts_with("punctuation.definition.string") {
            return CLASS_STRING;
        }
        if s.starts_with("constant.numeric") {
            return CLASS_NUMBER;
        }
        if s.starts_with("constant") {
            return CLASS_CONSTANT; // language/character/escape
        }
        if s.starts_with("keyword") || s.starts_with("storage.type")
            || s.starts_with("storage.modifier") || s.starts_with("variable.language")
        {
            return CLASS_KEYWORD;
        }
        if s.starts_with("entity.name.function")
            || s.starts_with("support.function")
            || s.starts_with("meta.function-call")
        {
            return CLASS_FUNCTION;
        }
        if s.starts_with("entity.name")
            || s.starts_with("support.type")
            || s.starts_with("support.class")
        {
            return CLASS_TYPE;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{detect_by_extension, syntax_set};

    fn engine_for(ext: &str) -> HighlightEngine {
        let syntax = detect_by_extension(ext).expect("syntax");
        let mut e = HighlightEngine::new();
        // Тесты: бюджет догонa не влияет на результат (детерминизм),
        // но debug-сборка под параллельной нагрузкой не обязана
        // укладываться в продовый кадр — снимаем ограничение.
        e.set_budget(Duration::from_secs(10));
        e.set_syntax(Some(syntax));
        e
    }

    fn doc(s: &str) -> Document {
        Document::from_text(s)
    }

    fn classes(e: &HighlightEngine, _doc: &Document, line: usize) -> Vec<(u32, u32, u8)> {
        e.spans(line)
            .expect("spans after ensure_window")
            .iter()
            .map(|s| (s.start, s.end, s.class))
            .collect()
    }

    /// Rust: `fn` — keyword, имя — function, строка — string, число.
    #[test]
    fn rust_line_classes() {
        let mut e = engine_for("rs");
        let d = doc("fn main() { let s = \"hi\"; }\n");
        e.ensure_window(&d, 0, 1);
        let spans = classes(&e, &d, 0);
        assert!(
            spans.iter().any(|(s, t, c)| *c == CLASS_KEYWORD && t > s),
            "keyword span present: {spans:?}"
        );
        assert!(
            spans.iter().any(|(_, _, c)| *c == CLASS_STRING),
            "string span present: {spans:?}"
        );
        // Диапазоны корректны: склеены, покрывают «fn» [0..2).
        let kw = spans.iter().find(|(_, _, c)| *c == CLASS_KEYWORD).unwrap();
        assert_eq!(kw.0, 0);
        assert_eq!(kw.1, 2);
    }

    /// Комментарий красится комментарием (Python `# ...`).
    #[test]
    fn python_comment_class() {
        let mut e = engine_for("py");
        let d = doc("# hello\nx = 1\n");
        e.ensure_window(&d, 0, 2);
        let spans = classes(&e, &d, 0);
        assert_eq!(spans, vec![(0, 7, CLASS_COMMENT)]);
    }

    /// Многострочный контекст: строки 2–3 целиком внутри docstring
    /// (грамматика Python красит его comment.block.documentation —
    /// стек скоупов переезжает через строки; ядро дизайна D12.1).
    #[test]
    fn multiline_state_carries_over() {
        let mut e = engine_for("py");
        let d = doc("def f():\n    \"\"\"doc\n    string\n    \"\"\"\n    return 1\n");
        e.ensure_window(&d, 0, 5);
        let l1 = classes(&e, &d, 1);
        assert!(l1.iter().any(|(s, t, c)| *c == CLASS_COMMENT && *t > *s), "{l1:?}");
        let l2 = classes(&e, &d, 2);
        // Вся строка 2 («    string») внутри docstring: один класс с
        // 0 до конца — состояние доехало со строки 1 (свежий разбор
        // дал бы plain/keyword, но не docstring-класс всей строки).
        assert_eq!(l2, vec![(0, 10, CLASS_COMMENT)], "{l2:?}");
    }

    /// Хвостовая строка БЕЗ терминатора подсвечивается (ловушка
    /// newlines-набора: движок дозаправляет `\n`).
    #[test]
    fn tail_line_without_newline_highlighted() {
        let mut e = engine_for("py");
        let d = doc("# tail comment");
        e.ensure_window(&d, 0, 1);
        assert_eq!(classes(&e, &d, 0), vec![(0, 14, CLASS_COMMENT)]);
    }

    /// Чекпоинты + прыжок: окно в середине файла после догонa
    /// подсвечивается теми же спанами, что и последовательный разбор.
    #[test]
    fn jump_matches_sequential_parse() {
        let mut e = engine_for("rs");
        let d = doc("fn a() {\n    // comment one\n}\nfn b() {\n    let x = 42;\n}\n");
        // Последовательный разбор окна [0..6)…
        e.ensure_window(&d, 0, 6);
        let sequential: Vec<_> = (0..6).map(|l| classes(&e, &d, l)).collect();
        // …и тот же файл с чистого движка окном [4..6) (прыжок):
        let mut e2 = engine_for("rs");
        e2.ensure_window(&d, 4, 2);
        for (l, expected) in sequential.iter().enumerate().take(6).skip(4) {
            assert_eq!(classes(&e2, &d, l), *expected, "line {l}");
        }
    }

    /// Догон с бюджетом: большой прыжок не закрывает окно за один вызов,
    /// но ДОГОНЯЕТ за несколько (прогрессивная подсветка). Возврат
    /// `ensure_window`: false пока окно не покрыто, true в конце —
    /// по нему рендер гоняет кадры догонa (баг «прыжок гасит цвет»).
    #[test]
    fn catchup_progresses_over_calls() {
        let mut e = engine_for("rs");
        // ~40 строк короткого rust-кода
        let text: String = (0..40).map(|i| format!("fn f{i}(x: u64) -> u64 {{ x + {i} }}\n")).collect();
        let d = doc(&text);
        // Нулевой бюджет: каждый вызов — минимум строк, окно далеко от
        // начала НЕ закрывается за один вызов (механика догонa).
        e.set_budget(Duration::ZERO);
        let mut saw_pending = false;
        let mut calls = 0;
        loop {
            let complete = e.ensure_window(&d, 35, 5);
            if complete || calls > 1000 {
                assert!(complete, "окно покрыто — ensure_window == true (calls={calls})");
                break;
            }
            saw_pending = true;
            calls += 1;
        }
        assert!(saw_pending, "нулевого бюджета не хватало хотя бы на один вызов");
        assert!(e.spans(35).is_some(), "caught up in {calls} calls");
        assert!(classes(&e, &d, 35).iter().any(|(_, _, c)| *c == CLASS_KEYWORD));
    }

    /// Нулевой бюджет не зависает: каждый вызов разбирает хотя бы одну
    /// строку (дедлайн ПОСЛЕ разбора) — notify-цикл рендера сходится.
    #[test]
    fn zero_budget_still_progresses() {
        let mut e = engine_for("rs");
        e.set_budget(Duration::ZERO);
        let text: String = (0..30).map(|i| format!("fn g{i}() {{ {i} }}\n")).collect();
        let d = doc(&text);
        let mut calls = 0;
        while !e.ensure_window(&d, 25, 5) {
            calls += 1;
            assert!(calls < 100, "нулевого бюджета хватает на {calls}+ вызовов?");
        }
        assert!(e.spans(25).is_some(), "окно в итоге покрыто за {calls} вызовов");
    }

    /// Фоновая ходка строит чекпоинты по всему файлу; после слияния
    /// прыжок вглубь закрывается ОДНИМ ensure_window (догон ≤
    /// CHECKPOINT_EVERY строк — кадры рендера больше не гоняют
    /// длинные догонs).
    #[test]
    fn background_walk_checkpoints_make_jumps_cheap() {
        let mut e = engine_for("rs");
        let text: String = (0..3000).map(|i| format!("fn f{i}(x: u64) -> u64 {{ x + {i} }}\n")).collect();
        let d = doc(&text);
        // Небольшой прогрев начала (как рендер первого экрана).
        e.set_budget(Duration::from_secs(1));
        e.ensure_window(&d, 0, 10);
        // Ходка от дальнего чекпоинта до конца файла.
        let mut w = BackgroundWalk::start(&e).expect("синтаксис есть");
        loop {
            let chunk = w.chunk(&d, Duration::from_secs(10));
            let done = chunk.done;
            e.merge_background(chunk, usize::MAX);
            if done {
                break;
            }
        }
        let cps = e.checkpoint_lines();
        assert!(cps.contains(&1024) && cps.contains(&2048), "чекпоинты: {cps:?}");
        // Прыжок на 2500: догон от чекпоинта 2048 — 452 строки,
        // один вызов с дефолтным бюджетом заведомо покрывает.
        e.set_budget(CATCH_UP_BUDGET);
        let complete = e.ensure_window(&d, 2500, 40);
        assert!(complete, "прыжок вглубь закрыт одним вызовом (чекпоинты)");
    }

    /// Слияние фоновой порции уважает правки: чекпоинты/спаны после
    /// первой строки правок (valid_upto) не вливаются.
    #[test]
    fn background_merge_respects_edits() {
        let mut e = engine_for("rs");
        let text: String = (0..2100).map(|i| format!("let v{i} = {i};\n")).collect();
        let d = doc(&text);
        let mut w = BackgroundWalk::start(&e).expect("синтаксис есть");
        let chunk = w.chunk(&d, Duration::from_secs(10));
        assert!(chunk.done, "2100 строк за один большой бюджет");
        // Правка на строке 5: всё, что глубже, невалидно.
        e.merge_background(chunk, 5);
        assert!(!e.checkpoint_lines().contains(&1024), "чекпоинт за правкой не влит");
        assert!(e.spans(500).is_none(), "спаны за правкой не влиты");
    }

    /// Инвалидация правки: спаны до строки правки живут, после — нет;
    /// повторное ensure_window пересчитывает.
    #[test]
    fn invalidate_from_keeps_prefix() {
        let mut e = engine_for("py");
        let d = doc("# a\n# b\n# c\n# d\n");
        e.ensure_window(&d, 0, 4);
        assert!(e.spans(2).is_some());
        e.invalidate_from(2);
        assert!(e.spans(1).is_some(), "до правки живёт");
        assert!(e.spans(2).is_none(), "после правки сброшено");
        assert!(e.spans(3).is_none());
        e.ensure_window(&d, 2, 2);
        assert_eq!(classes(&e, &d, 2), vec![(0, 3, CLASS_COMMENT)]);
    }

    /// Правка в середине: строки ниже пересчитываются с новым
    /// содержимым (спаны не протухают).
    #[test]
    fn invalidate_recomputes_after_edit() {
        use crate::transaction::Transaction;
        let mut e = engine_for("py");
        let mut d = doc("x = 1\ny = 2\n");
        e.ensure_window(&d, 0, 2);
        assert!(classes(&e, &d, 1).is_empty() || !classes(&e, &d, 1).is_empty()); // просто есть
        // Правка строки 1: `y = 2` → `# comment`
        let start = d.line_to_char(1);
        let tx = Transaction::from_edits(vec![crate::transaction::Edit {
            start,
            end: start + 5,
            text: "# comment".into(),
        }]);
        let line = d.char_to_line(start);
        d.apply(&tx);
        e.invalidate_from(line);
        e.ensure_window(&d, 0, 2);
        assert_eq!(classes(&e, &d, 1), vec![(0, 9, CLASS_COMMENT)]);
    }

    /// Кап длины: строка длиннее LINE_CHAR_CAP — без спанов (пустой
    /// кэш-маркер) и не роняет движок; соседние строки подсвечены.
    #[test]
    fn line_length_cap_skips_huge_line() {
        let mut e = engine_for("py");
        let huge = "# ".to_string() + &"x".repeat(LINE_CHAR_CAP + 100);
        let d = doc(&format!("# small\n{}\n# tail\n", huge));
        e.ensure_window(&d, 0, 3);
        assert_eq!(classes(&e, &d, 0), vec![(0, 7, CLASS_COMMENT)]);
        assert_eq!(e.spans(1).map(|s| s.len()), Some(0), "кап: пустые спаны");
        assert_eq!(classes(&e, &d, 2), vec![(0, 6, CLASS_COMMENT)]);
    }

    /// Пороговые значения спанов — char-координаты и в границах строки.
    #[test]
    fn spans_are_char_coords_in_bounds() {
        let mut e = engine_for("rs");
        let d = doc("fn f() { \"привет мир\" }\n"); // multibyte внутри строки
        e.ensure_window(&d, 0, 1);
        let n = d.line_content(0).chars().count();
        for s in e.spans(0).unwrap().iter() {
            assert!(s.start < s.end);
            assert!(s.end as usize <= n);
        }
    }

    /// set_syntax: смена языка сбрасывает кэш; тот же — нет; None — выкл.
    #[test]
    fn set_syntax_resets_and_none_disables() {
        let mut e = engine_for("py");
        let d = doc("# c\n");
        e.ensure_window(&d, 0, 1);
        assert!(e.spans(0).is_some());
        e.set_syntax(Some(detect_by_extension("rs").unwrap()));
        assert!(e.spans(0).is_none(), "кэш сброшен");
        assert_eq!(e.syntax_name(), Some("Rust"));
        // Тот же синтаксис — no-op (кэш не сбрасывает).
        e.ensure_window(&d, 0, 1);
        e.set_syntax(Some(detect_by_extension("rs").unwrap()));
        assert!(e.spans(0).is_some());
        e.set_syntax(None);
        assert_eq!(e.syntax_name(), None);
        assert!(e.spans.is_empty());
    }

    /// Чекпоинты не протухают: after pruning остаются только чекпоинты
    /// и окно (косвенно — через повторные окна без пересборки с нуля).
    #[test]
    fn states_prune_keeps_checkpoints_and_window() {
        let mut e = engine_for("py");
        let text: String = (0..50).map(|i| format!("# line {i}\n")).collect();
        let d = doc(&text);
        e.ensure_window(&d, 40, 10);
        // Чекпоинт на 0-строке нет, но restart-точка есть; новые окна
        // работают дальше от неё.
        e.ensure_window(&d, 0, 10);
        assert_eq!(classes(&e, &d, 5), vec![(0, 8, CLASS_COMMENT)]);
    }

    /// Автономный разбор двух строк подряд == разбор окна (API-эквив).
    #[test]
    fn standalone_parse_matches_window() {
        let syntax = detect_by_extension("rs").unwrap();
        let d = doc("fn a() { \"s\" }\n// tail\n");
        let mut e = engine_for("rs");
        e.ensure_window(&d, 0, 2);
        let (s0, st) = HighlightEngine::parse_line_standalone(
            syntax,
            "fn a() { \"s\" }\n",
            None,
        );
        let s0: Vec<_> = s0.iter().map(|s| (s.start, s.end, s.class)).collect();
        let (s1, _) = HighlightEngine::parse_line_standalone(syntax, "// tail\n", Some(&st));
        let s1: Vec<_> = s1.iter().map(|s| (s.start, s.end, s.class)).collect();
        assert_eq!(s0, classes(&e, &d, 0));
        assert_eq!(s1, classes(&e, &d, 1));
    }

    // ---- CSV/TSV (D12.4): колонки чередуются цветами ----

    fn csv_engine() -> HighlightEngine {
        let syntax = detect_by_extension("csv").expect("csv syntax");
        let mut e = HighlightEngine::new();
        e.set_budget(Duration::from_secs(10));
        e.set_syntax(Some(syntax));
        assert!(matches!(e.syntax_name(), Some("CSV")));
        e
    }

    /// Колонки чередуются цветами: 3 колонки → классы A, B, C по
    /// диапазонам полей; соседние колонки всегда разного цвета.
    #[test]
    fn csv_columns_alternate_classes() {
        let mut e = csv_engine();
        let d = doc("name,count,note\nalpha,42,ok\n");
        e.ensure_window(&d, 0, 2);
        assert_eq!(
            classes(&e, &d, 0),
            vec![
                (0, 4, CLASS_COLUMN_A),
                (5, 10, CLASS_COLUMN_B),
                (11, 15, CLASS_COLUMN_C),
            ]
        );
        assert_eq!(
            classes(&e, &d, 1),
            vec![
                (0, 5, CLASS_COLUMN_A),
                (6, 8, CLASS_COLUMN_B),
                (9, 11, CLASS_COLUMN_C),
            ]
        );
    }

    /// Ротация: 5-я колонка снова цвет A; соседние всегда различимы.
    #[test]
    fn csv_four_color_rotation() {
        let mut e = csv_engine();
        let d = doc("a,b,c,d,e\n");
        e.ensure_window(&d, 0, 1);
        assert_eq!(
            classes(&e, &d, 0),
            vec![
                (0, 1, CLASS_COLUMN_A),
                (2, 3, CLASS_COLUMN_B),
                (4, 5, CLASS_COLUMN_C),
                (6, 7, CLASS_COLUMN_D),
                (8, 9, CLASS_COLUMN_A),
            ]
        );
    }

    /// Кавычки RFC-4180: запятая внутри `"…"` не делит поле; `""` —
    /// экранированная кавычка внутри поля.
    #[test]
    fn csv_quoted_separator_and_escaped_quotes() {
        let mut e = csv_engine();
        let d = doc("\"a,b\",x\n\"he said \"\"hi\"\"\",y\n");
        e.ensure_window(&d, 0, 2);
        // Строка 0: поле0 = "a,b" [0..5) — ОДНА колонка, поле1 = x.
        assert_eq!(
            classes(&e, &d, 0),
            vec![(0, 5, CLASS_COLUMN_A), (6, 7, CLASS_COLUMN_B)]
        );
        // Строка 1: поле0 = `"he said ""hi"""` — 16 char, поле1 = y.
        assert_eq!(
            classes(&e, &d, 1),
            vec![(0, 16, CLASS_COLUMN_A), (17, 18, CLASS_COLUMN_B)]
        );
    }

    /// TSV: таб-файл (большинство разделителей — табы).
    #[test]
    fn tsv_tabs_detected_per_line() {
        let mut e = csv_engine();
        let d = doc("a\tb\tc\n1\t2\t3\n");
        e.ensure_window(&d, 0, 2);
        assert_eq!(
            classes(&e, &d, 0),
            vec![
                (0, 1, CLASS_COLUMN_A),
                (2, 3, CLASS_COLUMN_B),
                (4, 5, CLASS_COLUMN_C),
            ]
        );
    }

    /// Пустые поля пропускаются, чередование цветов сохраняется
    /// (индекс колонки считает и пустые).
    #[test]
    fn csv_empty_fields_keep_rotation() {
        let mut e = csv_engine();
        let d = doc("a,,c\n");
        e.ensure_window(&d, 0, 1);
        assert_eq!(
            classes(&e, &d, 0),
            vec![(0, 1, CLASS_COLUMN_A), (3, 4, CLASS_COLUMN_C)]
        );
    }

    /// CSV — stateless: состояние разбора не меняется (многострочность
    /// не нужна), кап длины строки действует как для остальных.
    #[test]
    fn csv_stateless_and_cap() {
        let mut e = csv_engine();
        let huge = "x,".to_string() + &"y".repeat(LINE_CHAR_CAP + 10);
        let d = doc(&format!("a,b\n{}\nc,d\n", huge));
        e.ensure_window(&d, 0, 3);
        assert_eq!(classes(&e, &d, 0).len(), 2);
        assert_eq!(e.spans(1).map(|s| s.len()), Some(0), "кап длины");
        assert_eq!(classes(&e, &d, 2).len(), 2, "строка после капа красится");
    }

    /// Детект .csv/.tsv → грамматика CSV (синтакс-модуль).
    #[test]
    fn csv_detected_by_extension() {
        assert_eq!(detect_by_extension("csv").unwrap().name, "CSV");
        assert_eq!(detect_by_extension("tsv").unwrap().name, "CSV");
        assert_eq!(detect_by_extension("CSV").unwrap().name, "CSV");
    }

    /// Память кэшей не растёт без_boundа: окно в конце большого файла
    /// после множества прыжков держит кэш в пределах SPANS_LIMIT*2.
    #[test]
    fn spans_cache_bounded() {
        let mut e = engine_for("py");
        let text: String = (0..3000).map(|i| format!("# line {i}\n")).collect();
        let d = doc(&text);
        // Прыжки по всему файлу окнами — кэш под контролем.
        for first in [0usize, 1500, 2990, 500, 2500] {
            e.ensure_window(&d, first, 40);
        }
        assert!(e.spans.len() <= SPANS_LIMIT * 2, "spans: {}", e.spans.len());
    }

    /// Компонентный бенчмарк подсветки (ignored; D12.2):
    /// cargo test --release -p editor-core highlight -- --ignored
    #[test]
    #[ignore = "бенчмарк (D12.2): инициализация набора, скорость разбора, капы"]
    fn bench_highlight_components() {
        let t = Instant::now();
        let set = syntax_set();
        let init_ms = t.elapsed().as_secs_f64() * 1000.0;
        println!("syntax_set init: {init_ms:.1} ms ({} syntaxes)", set.syntaxes().len());

        let doc_text: String = (0..290_000)
            .map(|n| format!("{:6}    call_{n:06}(arg1, arg2) {{ if (deep) {{ trace(); }} }}\n", n % 9 * 4))
            .collect();
        let d = Document::from_text(&doc_text);
        let syntax = detect_by_extension("rs").unwrap();

        // Первый parse — компиляция regex'ов синтаксиса (onig OnceCell).
        let t = Instant::now();
        let mut st = LineState { parse: ParseState::new(syntax), scopes: ScopeStack::new() };
        let mut e = HighlightEngine::new();
        e.set_syntax(Some(syntax));
        e.parse_into(&d, 0, &mut st);
        println!("first parse (regex compile): {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);

        // Скорость разбора: 100k строк от чекпоинта.
        let t = Instant::now();
        let mut n = 0usize;
        let deadline = t + Duration::from_secs(2);
        while Instant::now() < deadline && n < 100_000 {
            e.parse_into(&d, n, &mut st);
            n += 1;
        }
        let el = t.elapsed().as_secs_f64();
        println!("parse rate: {n} lines in {el:.2}s = {:.1}k lines/s, {:.2} µs/line", n as f64 / el / 1000.0, el * 1e6 / n as f64);

        // Окно 60 строк с тёплым кэшем regex'ов.
        let mut e2 = HighlightEngine::new();
        e2.set_syntax(Some(syntax));
        let t = Instant::now();
        e2.ensure_window(&d, 145_000, 60);
        println!("cold window 60 lines (jump to 145k, budget-capped): {:.2} ms", t.elapsed().as_secs_f64() * 1000.0);
        let mut calls = 0;
        let t = Instant::now();
        while e2.spans(145_000).is_none() {
            e2.ensure_window(&d, 145_000, 60);
            calls += 1;
        }
        println!("catchup to 145k: {calls} calls, {:.1} ms total", t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        e2.ensure_window(&d, 145_000, 60);
        println!("warm window 60 lines: {:.3} ms", t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        e2.ensure_window(&d, 145_000, 60);
        println!("warm window steady state: {:.3} ms", t.elapsed().as_secs_f64() * 1000.0);

        // Патологическая строка: 1 МБ в одной строке — БЕЗ капа
        // (напрямую parse_line — движок такие строки не кормит).
        let huge = "let x = \"".to_string() + &"a".repeat(1_000_000) + "\";\n";
        let mut sh = ParseState::new(syntax);
        let t = Instant::now();
        let ops = sh.parse_line(&huge, set).expect("parse");
        println!(
            "1MB single line UNCAPPED parse: {:.1} ms ({} ops) — обоснование капа",
            t.elapsed().as_secs_f64() * 1000.0,
            ops.len()
        );
        // Через движок (кап): мгновенно, спанов нет.
        let dh = Document::from_text(&huge);
        let mut eh = HighlightEngine::new();
        eh.set_syntax(Some(syntax));
        let t = Instant::now();
        eh.ensure_window(&dh, 0, 1);
        println!("1MB single line via engine (cap): {:.3} ms (spans: {:?})", t.elapsed().as_secs_f64() * 1000.0, eh.spans(0).map(|s| s.len()));
        // Кап 8 КБ для сравнения.
        let capped = "let x = \"".to_string() + &"a".repeat(LINE_CHAR_CAP) + "\";\n";
        let dc = Document::from_text(&capped);
        let mut ec = HighlightEngine::new();
        ec.set_syntax(Some(syntax));
        let t = Instant::now();
        ec.ensure_window(&dc, 0, 1);
        println!("8KB line parse: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0);
    }
}
