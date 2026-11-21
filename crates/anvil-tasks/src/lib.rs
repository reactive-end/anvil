//! Conflict-free team task ledger and consolidated tasks.json builder.

/// Crate version marker for workspace smoke checks.
pub fn crate_name() -> &'static str {
    let _ = anvil_core::traits::Tier::Tier0;
    "anvil-tasks"
}
