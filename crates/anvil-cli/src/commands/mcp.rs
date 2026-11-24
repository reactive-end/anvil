//! `anvil mcp`: starts the unified stdio MCP server (or `--proxy` Code Mode gateway).

use std::path::Path;

use anyhow::Result;

/// Runs the buffered stdio JSON-RPC 2.0 MCP server (or Code Mode proxy gateway) for `root`.
pub fn execute(root: &Path, proxy: bool) -> Result<()> {
    if proxy {
        anvil_mcp::run_proxy_server(root)
    } else {
        anvil_mcp::run_stdio_server(root)
    }
}
