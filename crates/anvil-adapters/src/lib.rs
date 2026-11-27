//! Thin adapters for Antigravity, Claude Code, Cursor, Windsurf, and Codex hooks, configs, docs, and skills.

pub mod antigravity;
pub mod architecture;
pub mod claude_code;
pub mod codex;
pub mod contract;
pub mod cursor;
pub mod db_mcp;
pub mod inbox;
pub(crate) mod managed_mcp;
pub mod sync;
pub mod templates;
pub mod web_qa;
pub mod windsurf;

/// Crate identifier for workspace smoke checks.
pub fn crate_name() -> &'static str {
    "anvil-adapters"
}
