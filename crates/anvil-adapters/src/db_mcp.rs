//! Registers the Google MCP Toolbox for Databases server for agents when `db.mcp` is on, so they can
//! query the application database themselves, and removes the entry again when it is turned off.
//!
//! Only the entry Anvil writes is ever touched; an `anvil-db` server a user configured with other
//! options stays as it is. Calls with a `sql` field already pass through the security guard.

use std::path::{Path, PathBuf};

use anvil_core::config::db::DbConfig;
use anvil_core::config::{AdapterTarget, AnvilConfig};
use anyhow::Result;
use serde_json::Value;

use crate::claude_code::mcp_servers::tool_pattern;
use crate::managed_mcp::{self, ManagedServer};

/// Name of the MCP server entry, which also names its tools (`mcp__anvil-db__*`).
pub const DB_SERVER: &str = "anvil-db";

/// Arguments that serve the tools file over MCP stdio.
pub fn mcp_args(db: &DbConfig) -> Vec<String> {
    ["--stdio".to_owned(), "--config".to_owned(), db.config.clone()].to_vec()
}

/// The `mcpServers` entry JSON clients read. Clients start MCP servers from the repository root,
/// so the relative binary and tools-file paths of `db` work as they are.
pub fn server_entry(db: &DbConfig) -> Value {
    managed_server(db).entry()
}

/// The executor allowlist entry that grants every tool of the server.
pub fn executor_tool() -> String {
    tool_pattern(DB_SERVER)
}

/// `true` when `args` look like the command Anvil writes (any tools file).
fn is_managed_args(args: &[&str]) -> bool {
    args.contains(&"--stdio") && args.contains(&"--config")
}

fn managed_server(db: &DbConfig) -> ManagedServer {
    ManagedServer {
        name: DB_SERVER,
        command: db.toolbox.clone(),
        args: mcp_args(db),
        recognizes: is_managed_args,
    }
}

/// Brings every adapter's MCP config and the executor allowlist in line with `db.mcp`.
/// Returns the files that changed.
pub fn apply(repo_root: &Path, target: AdapterTarget) -> Result<Vec<PathBuf>> {
    let mut config = AnvilConfig::load_or_default(repo_root)?;
    let server = managed_server(&config.db);
    let enabled = config.db.mcp;
    managed_mcp::apply(repo_root, target, &mut config, &server, enabled)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_entry_serves_the_tools_file_over_stdio() {
        let db = DbConfig { toolbox: ".tools/toolbox.exe".to_owned(), ..DbConfig::default() };
        assert_eq!(
            server_entry(&db),
            json!({
                "command": ".tools/toolbox.exe",
                "args": ["--stdio", "--config", ".harness/db/tools.yaml"]
            })
        );
    }

    #[test]
    fn only_the_entry_anvil_writes_counts_as_managed() {
        let server = managed_server(&DbConfig::default());
        assert!(server.is_managed_entry(&server_entry(&DbConfig::default())));
        let user = json!({ "command": "toolbox", "args": ["--prebuilt", "postgres", "--stdio"] });
        assert!(!server.is_managed_entry(&user));
        assert!(!server.is_managed_entry(&json!({ "command": "toolbox" })));
    }
}
