//! Дамп расширений всех поддерживаемых синтаксисов (для упаковки:
//! packaging/Info.plist → CFBundleDocumentTypes, «Открыть в приложении»).
//! Запуск: cargo run -p editor-core --example dump_exts
//!
//! При добавлении языков в набор — перегенерировать список в plist.

fn main() {
    let set = editor_core::syntax::syntax_set();
    let mut exts: Vec<String> = set
        .syntaxes()
        .iter()
        .flat_map(|s| s.file_extensions.iter().cloned())
        .collect();
    // Явные дополнения: Plain Text/CSV/TOML/TS/Shell — грамматики набора
    // не всегда несут «пользовательские» расширения.
    exts.extend(
        [
            "txt", "text", "log", "csv", "tsv", "toml", "tml", "ts", "tsx", "mts", "cts", "sh",
            "bash", "zsh",
        ]
        .map(String::from),
    );
    exts.sort();
    exts.dedup();
    // Только разумные расширения (без служебных вроде числовых).
    let exts: Vec<String> = exts
        .into_iter()
        .filter(|e| e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .collect();
    for e in &exts {
        println!("{e}");
    }
    eprintln!("total: {}", exts.len());
}
