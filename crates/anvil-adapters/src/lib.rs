//! Thin adapters for Antigravity and Claude Code hooks, configs, docs, and skills.

pub mod antigravity;
mod atomic_fs;
pub mod claude_code;
pub mod contract;
pub mod inbox;
pub mod sync;
pub mod templates;

/// Crate identifier for workspace smoke checks.
pub fn crate_name() -> &'static str {
    "anvil-adapters"
}
