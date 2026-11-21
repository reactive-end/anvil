//! Thin adapters for Antigravity and Claude Code hooks/configs/skills.

/// Crate version marker for workspace smoke checks.
pub fn crate_name() -> &'static str {
    let _ = anvil_core::traits::Tier::Tier0;
    "anvil-adapters"
}
