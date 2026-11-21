//! Tree-sitter AST parsing and Tier 0 deterministic rules.

/// Crate version marker for workspace smoke checks.
pub fn crate_name() -> &'static str {
    let _ = anvil_core::traits::Tier::Tier0;
    "anvil-ast"
}
