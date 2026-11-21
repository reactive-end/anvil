//! `anvil mcp`: starts the unified stdio MCP server.

use std::path::Path;

use anyhow::Result;

/// Runs the buffered stdio JSON-RPC 2.0 MCP server for `root`.
pub fn execute(root: &Path) -> Result<()> {
    anvil_mcp::run_stdio_server(root)
}
