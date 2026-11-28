//! Registers Google's Chrome DevTools MCP server for agents when the web QA driver is
//! `chrome_dev_tools`, and removes the entry again when the driver changes.
//!
//! Only the entry Anvil writes is ever touched; a `chrome-devtools` server a user configured with
//! other options stays as it is.

use std::path::{Path, PathBuf};

use anvil_core::config::orchestrator::OrchestratorConfig;
use anvil_core::config::{AdapterTarget, AnvilConfig, WebQaDriver};
use anyhow::Result;
use serde_json::Value;

use crate::claude_code::mcp_servers::tool_pattern;
use crate::managed_mcp::{self, ManagedServer};

/// Pinned `chrome-devtools-mcp` release (Apache-2.0).
pub const CHROME_DEVTOOLS_MCP_VERSION: &str = "1.14.0";

/// Name of the MCP server entry, which also names its tools (`mcp__chrome-devtools__*`).
pub const CHROME_DEVTOOLS_SERVER: &str = "chrome-devtools";

/// Arguments of the `chrome-devtools-mcp` process. Headless with a throwaway profile, and no
/// usage statistics or CrUX lookups: the lookups would send the URLs of local apps to Google.
pub fn mcp_args() -> Vec<String> {
    [
        "-y".to_owned(),
        format!("chrome-devtools-mcp@{CHROME_DEVTOOLS_MCP_VERSION}"),
        "--isolated".to_owned(),
        "--headless".to_owned(),
        "--no-usage-statistics".to_owned(),
        "--no-performance-crux".to_owned(),
    ]
    .to_vec()
}

/// Program and arguments that start the server. Native Windows clients cannot spawn `npx`
/// (a `.cmd` shim) directly, so it goes through `cmd /c`.
pub fn launch_command(windows: bool) -> (String, Vec<String>) {
    if windows {
        let mut args = vec!["/c".to_owned(), "npx".to_owned()];
        args.extend(mcp_args());
        ("cmd".to_owned(), args)
    } else {
        ("npx".to_owned(), mcp_args())
    }
}

/// The `mcpServers` entry JSON clients read.
pub fn server_entry(windows: bool) -> Value {
    managed_server(windows).entry()
}

/// The executor allowlist entry that grants every tool of the server.
pub fn executor_tool() -> String {
    tool_pattern(CHROME_DEVTOOLS_SERVER)
}

/// `true` when `args` look like the command Anvil writes (any pinned version).
fn is_managed_args(args: &[&str]) -> bool {
    let package = args.iter().any(|arg| arg.starts_with("chrome-devtools-mcp@"));
    package && args.contains(&"--isolated") && args.contains(&"--no-performance-crux")
}

pub(crate) fn managed_server(windows: bool) -> ManagedServer {
    let (command, args) = launch_command(windows);
    ManagedServer { name: CHROME_DEVTOOLS_SERVER, command, args, recognizes: is_managed_args }
}

/// Grants or revokes the server's tools for executors. Returns whether the list changed.
pub fn set_executor_tool(orchestrator: &mut OrchestratorConfig, granted: bool) -> bool {
    managed_mcp::set_executor_tool(orchestrator, &executor_tool(), granted)
}

/// Brings every adapter's MCP config and the executor allowlist in line with `qa_driver.web`.
/// Returns the files that changed.
pub fn apply(repo_root: &Path, target: AdapterTarget) -> Result<Vec<PathBuf>> {
    let mut config = AnvilConfig::load_or_default(repo_root)?;
    let enabled = config.qa_driver.web == WebQaDriver::ChromeDevTools;
    let server = managed_server(cfg!(windows));
    managed_mcp::apply(repo_root, target, &mut config, &server, enabled)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn entry_carries_the_pinned_version_and_the_privacy_flags() {
        let (command, args) = launch_command(false);
        assert_eq!(command, "npx");
        assert_eq!(args[0], "-y");
        assert_eq!(args[1], format!("chrome-devtools-mcp@{CHROME_DEVTOOLS_MCP_VERSION}"));
        for flag in ["--isolated", "--headless", "--no-usage-statistics", "--no-performance-crux"] {
            assert!(args.iter().any(|a| a == flag), "missing {flag}");
        }
    }

    #[test]
    fn windows_goes_through_cmd() {
        let (command, args) = launch_command(true);
        assert_eq!(command, "cmd");
        assert_eq!(&args[..3], ["/c", "npx", "-y"]);
        assert!(managed_server(true).is_managed_entry(&server_entry(true)));
    }

    #[test]
    fn only_the_entry_anvil_writes_counts_as_managed() {
        let managed = managed_server(false);
        assert!(managed.is_managed_entry(&server_entry(false)));
        let user = json!({ "command": "npx", "args": ["chrome-devtools-mcp@latest", "--browser-url=http://127.0.0.1:9222"] });
        assert!(!managed.is_managed_entry(&user));
        assert!(!managed.is_managed_entry(&json!({ "command": "chrome-devtools-mcp" })));
    }

    #[test]
    fn executor_grant_is_added_and_removed_once() {
        let mut orchestrator = OrchestratorConfig::default();
        assert!(set_executor_tool(&mut orchestrator, true));
        assert!(!set_executor_tool(&mut orchestrator, true));
        assert_eq!(orchestrator.executor_tools, vec!["mcp__chrome-devtools__*".to_owned()]);
        assert!(set_executor_tool(&mut orchestrator, false));
        assert!(!set_executor_tool(&mut orchestrator, false));
        assert!(orchestrator.executor_tools.is_empty());
    }
}
