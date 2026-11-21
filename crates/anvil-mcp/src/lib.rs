//! Unified stdio MCP server exposing graph_* and mem_* tools.

/// Crate version marker for workspace smoke checks.
pub fn crate_name() -> &'static str {
    let _ = anvil_core::traits::Tier::Tier0;
    let _ = anvil_graph::crate_name();
    let _ = anvil_memory::crate_name();
    "anvil-mcp"
}
