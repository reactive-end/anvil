//! Reading env files by key name only: values are never kept, so they can never be printed.

use std::path::Path;

/// Reads a text file, dropping a UTF-8 BOM. `None` when it cannot be read.
pub fn read_text(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(text.strip_prefix('\u{feff}').map_or(text.clone(), str::to_owned))
}

/// Key names defined in env-file `text`, in file order. Handles BOM, CRLF, comments and `export`.
pub fn env_keys(text: &str) -> Vec<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut keys: Vec<String> = Vec::new();
    for line in text.split(['\n', '\r']) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, _value)) = line.split_once('=') else { continue };
        let key = key.trim();
        if is_key(key) && !keys.iter().any(|k| k == key) {
            keys.push(key.to_owned());
        }
    }
    keys
}

fn is_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Keys of the env file at `path`; `None` when it does not exist or cannot be read.
pub fn keys_of_file(path: &Path) -> Option<Vec<String>> {
    read_text(path).map(|text| env_keys(&text))
}
