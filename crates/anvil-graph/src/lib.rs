//! Incremental code graph engine and blast-radius analyzer.

/// Crate version marker for workspace smoke checks.
pub fn crate_name() -> &'static str {
    let _ = anvil_core::traits::Tier::Tier1;
    let _ = anvil_ast::crate_name();
    "anvil-graph"
}
