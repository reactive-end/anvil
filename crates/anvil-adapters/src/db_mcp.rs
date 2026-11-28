//! Registers the Google MCP Toolbox for Databases server for agents when `db.mcp` is on, so they can
//! query the application database themselves, and removes the entry again when it is turned off.
//!
//! Only the entry Anvil writes is ever touched; an `anvil-db` server a user configured with other
//! options stays as it is. Calls with a `sql` field already pass through the security guard.

use std::path::{Path, PathBuf};

use anvil_core::config::{AdapterTarget, AnvilConfig};
use anyhow::Result;
use serde_json::Value;

use crate::claude_code::mcp_servers::tool_pattern;
use crate::managed_mcp::{self, ManagedServer};

/// Name of the MCP server entry, which also names its tools (`mcp__anvil-db__*`).
pub const DB_SERVER: &str = "anvil-db";

/// Arguments of `anvil db serve`, which starts Toolbox on stdio with the project's connection
/// values set, so the MCP config never holds credentials.
pub fn mcp_args() -> Vec<String> {
    ["db".to_owned(), "serve".to_owned()].to_vec()
}

/// The `mcpServers` entry JSON clients read. Clients start MCP servers from the repository root,
/// which is where `anvil db serve` finds `.harness/config.json`.
pub fn server_entry() -> Value {
    managed_server().entry()
}

/// The executor allowlist entry that grants every tool of the server.
pub fn executor_tool() -> String {
    tool_pattern(DB_SERVER)
}

/// `true` when `args` look like a command Anvil writes: `anvil db serve`, or the direct
/// `toolbox --stdio --config <file>` entry of earlier versions (replaced on the next sync).
fn is_managed_args(args: &[&str]) -> bool {
    args.ends_with(&["db", "serve"]) || (args.contains(&"--stdio") && args.contains(&"--config"))
}

fn managed_server() -> ManagedServer {
    ManagedServer {
        name: DB_SERVER,
        command: "anvil".to_owned(),
        args: mcp_args(),
        recognizes: is_managed_args,
    }
}

/// Brings every adapter's MCP config and the executor allowlist in line with `db.mcp`.
/// Returns the files that changed.
pub fn apply(repo_root: &Path, target: AdapterTarget) -> Result<Vec<PathBuf>> {
    let mut config = AnvilConfig::load_or_default(repo_root)?;
    let server = managed_server();
    let enabled = config.db.mcp;
    managed_mcp::apply(repo_root, target, &mut config, &server, enabled)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_entry_runs_anvil_db_serve() {
        assert_eq!(server_entry(), json!({ "command": "anvil", "args": ["db", "serve"] }));
    }

    #[test]
    fn only_the_entry_anvil_writes_counts_as_managed() {
        let server = managed_server();
        assert!(server.is_managed_entry(&server_entry()));
        let legacy = json!({ "command": "toolbox", "args": ["--stdio", "--config", "t.yaml"] });
        assert!(server.is_managed_entry(&legacy), "older entries are replaced, not left behind");
        let user = json!({ "command": "toolbox", "args": ["--prebuilt", "postgres", "--stdio"] });
        assert!(!server.is_managed_entry(&user));
        assert!(!server.is_managed_entry(&json!({ "command": "toolbox" })));
    }
}
