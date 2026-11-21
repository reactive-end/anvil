//! Hybrid visual QA driver for Web DOM assertions and mobile view trees.

/// Crate version marker for workspace smoke checks.
pub fn crate_name() -> &'static str {
    let _ = anvil_core::traits::Tier::Tier2;
    "anvil-qa"
}
