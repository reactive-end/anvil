//! Shell command inspection shared by the hook guards: lexing, wrapper expansion, and risk rules.

pub mod cloud;
pub mod destructive;
pub mod expand;
pub mod lex;
pub mod secrets;
pub mod targets;

use anvil_core::config::security_guard::GuardCategory;

/// Why a command is risky and which guard category decides what happens to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Risk {
    pub category: GuardCategory,
    pub desc: &'static str,
}

impl Risk {
    pub const fn new(category: GuardCategory, desc: &'static str) -> Self {
        Self { category, desc }
    }
}

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
