//! Local-first persistent memory with SQLite FTS5 and Git chunks.

/// Crate version marker for workspace smoke checks.
pub fn crate_name() -> &'static str {
    let _ = anvil_core::traits::FreshnessState::Fresh;
    "anvil-memory"
}
