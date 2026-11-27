//! Registers Google's Chrome DevTools MCP server for agents when the web QA driver is
//! `chrome_dev_tools`, and removes the entry again when the driver changes.
//!
//! Only the entry Anvil writes is ever touched; a `chrome-devtools` server a user configured with
//! other options stays as it is.

mod codex_toml;

use std::path::{Path, PathBuf};

use anvil_core::config::orchestrator::OrchestratorConfig;
use anvil_core::config::{AdapterTarget, AnvilConfig, WebQaDriver};
use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::antigravity::config::{ensure_object_field, read_json_object};
use crate::claude_code::mcp_servers::tool_pattern;
use crate::sync::{should_sync_codex, should_sync_cursor, should_sync_windsurf, write_if_changed};
use crate::{antigravity, claude_code, codex, cursor, windsurf};

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
    let (command, args) = launch_command(windows);
    json!({ "command": command, "args": args })
}

/// The executor allowlist entry that grants every tool of the server.
pub fn executor_tool() -> String {
    tool_pattern(CHROME_DEVTOOLS_SERVER)
}

/// `true` when `args` look like the command Anvil writes (any pinned version).
pub(crate) fn is_managed_args<'a>(args: impl IntoIterator<Item = &'a str>) -> bool {
    let (mut package, mut isolated, mut no_crux) = (false, false, false);
    for arg in args {
        package |= arg.starts_with("chrome-devtools-mcp@");
        isolated |= arg == "--isolated";
        no_crux |= arg == "--no-performance-crux";
    }
    package && isolated && no_crux
}

fn is_managed_entry(entry: &Value) -> bool {
    let args = entry.get("args").and_then(Value::as_array);
    args.is_some_and(|args| is_managed_args(args.iter().filter_map(Value::as_str)))
}

/// Adds or removes the managed server in a JSON file with an `mcpServers` object.
/// Returns whether the file changed.
pub fn merge_json_servers(path: &Path, label: &str, enabled: bool, windows: bool) -> Result<bool> {
    if !enabled && !path.exists() {
        return Ok(false);
    }
    let mut root = read_json_object(path)?;
    let servers = ensure_object_field(&mut root, "mcpServers")?;
    if servers.get(CHROME_DEVTOOLS_SERVER).is_some_and(|entry| !is_managed_entry(entry)) {
        return Ok(false);
    }
    if enabled {
        servers.insert(CHROME_DEVTOOLS_SERVER.to_owned(), server_entry(windows));
    } else if servers.remove(CHROME_DEVTOOLS_SERVER).is_none() {
        return Ok(false);
    }
    let formatted = serde_json::to_string_pretty(&Value::Object(root))
        .with_context(|| format!("failed to serialize {label}"))?;
    write_if_changed(path, &format!("{formatted}\n"))
}

/// Grants or revokes the server's tools for executors. Returns whether the list changed.
pub fn set_executor_tool(orchestrator: &mut OrchestratorConfig, granted: bool) -> bool {
    let pattern = executor_tool();
    let present = orchestrator.executor_tools.contains(&pattern);
    if granted && !present {
        orchestrator.executor_tools.push(pattern);
    } else if !granted && present {
        orchestrator.executor_tools.retain(|tool| *tool != pattern);
    } else {
        return false;
    }
    true
}

/// Brings every adapter's MCP config and the executor allowlist in line with `qa_driver.web`.
/// Returns the files that changed.
pub fn apply(repo_root: &Path, target: AdapterTarget) -> Result<Vec<PathBuf>> {
    let mut config = AnvilConfig::load_or_default(repo_root)?;
    let enabled = config.qa_driver.web == WebQaDriver::ChromeDevTools;
    let windows = cfg!(windows);
    let mut written = Vec::new();
    let mut json_targets: Vec<(PathBuf, &str)> = Vec::new();
    if target.includes_claude() {
        json_targets.push((claude_code::config::mcp_config_path(repo_root), ".mcp.json"));
    }
    if target.includes_antigravity() {
        json_targets
            .push((antigravity::config::mcp_config_path(repo_root), ".agents/mcp_config.json"));
    }
    if should_sync_cursor(repo_root, target) {
        json_targets.push((cursor::mcp_config_path(repo_root), ".cursor/mcp.json"));
    }
    if should_sync_windsurf(repo_root, target) {
        json_targets.push((windsurf::mcp_config_path(repo_root), ".windsurf/mcp_config.json"));
    }
    for (path, label) in json_targets {
        if merge_json_servers(&path, label, enabled, windows)? {
            written.push(path);
        }
    }
    if should_sync_codex(repo_root, target)
        && codex_toml::merge_codex_server(&codex::config_path(repo_root), enabled, windows)?
    {
        written.push(codex::config_path(repo_root));
    }
    written.extend(sync_executor_grant(repo_root, &mut config, target, enabled)?);
    Ok(written)
}

/// Updates `orchestrator.executor_tools` and regenerates the agents when it changed. A server
/// the user configured by hand keeps its grant when the driver moves away.
fn sync_executor_grant(
    repo_root: &Path,
    config: &mut AnvilConfig,
    target: AdapterTarget,
    enabled: bool,
) -> Result<Vec<PathBuf>> {
    let granted = enabled || user_server_remains(repo_root);
    let Some(orchestrator) = config.orchestrator.as_mut() else {
        return Ok(Vec::new());
    };
    if !set_executor_tool(orchestrator, granted) {
        return Ok(Vec::new());
    }
    let pair = orchestrator.clone();
    config.save(repo_root)?;
    let mut written = vec![AnvilConfig::path(repo_root)];
    if target.includes_claude() {
        written.extend(claude_code::agents::sync_agents(repo_root, &pair)?);
    }
    Ok(written)
}

/// `true` when some repository MCP config still defines a `chrome-devtools` server.
fn user_server_remains(repo_root: &Path) -> bool {
    let files = [
        claude_code::config::mcp_config_path(repo_root),
        antigravity::config::mcp_config_path(repo_root),
        cursor::mcp_config_path(repo_root),
        windsurf::mcp_config_path(repo_root),
    ];
    files.iter().any(|path| {
        read_json_object(path).ok().is_some_and(|root| {
            root.get("mcpServers").and_then(|s| s.get(CHROME_DEVTOOLS_SERVER)).is_some()
        })
    }) || codex_toml::codex_server_present(&codex::config_path(repo_root))
}

#[cfg(test)]
mod tests {
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
        assert!(is_managed_entry(&server_entry(true)));
    }

    #[test]
    fn only_the_entry_anvil_writes_counts_as_managed() {
        assert!(is_managed_entry(&server_entry(false)));
        let user = json!({ "command": "npx", "args": ["chrome-devtools-mcp@latest", "--browser-url=http://127.0.0.1:9222"] });
        assert!(!is_managed_entry(&user));
        assert!(!is_managed_entry(&json!({ "command": "chrome-devtools-mcp" })));
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
