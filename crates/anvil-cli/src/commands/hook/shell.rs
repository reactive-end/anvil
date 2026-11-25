//! Shell command inspection shared by the hook guards: lexing, wrapper expansion, and risk rules.

pub mod destructive;
pub mod expand;
pub mod lex;
pub mod secrets;
pub mod targets;

/// Normalizes an executable token to its lowercase base name without a Windows extension.
pub fn command_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let lower = base.to_ascii_lowercase();
    [".exe", ".cmd", ".bat", ".com"]
        .iter()
        .find_map(|ext| lower.strip_suffix(ext))
        .unwrap_or(&lower)
        .to_owned()
}
