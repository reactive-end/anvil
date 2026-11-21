//! Tier 2 dependency health auditing across npm, PyPI, pub.dev, and Maven.

/// Crate version marker for workspace smoke checks.
pub fn crate_name() -> &'static str {
    let _ = anvil_core::traits::Tier::Tier2;
    "anvil-deps"
}
