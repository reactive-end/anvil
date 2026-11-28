//! MCP servers Anvil registers for agents on request (Chrome DevTools, the database Toolbox):
//! one entry in every adapter's MCP config and a grant for executors, removed again when switched off.
//!
//! Only the entry Anvil writes is ever touched; a server of the same name that a user configured
//! with other options stays as it is.

mod codex_toml;

use std::path::{Path, PathBuf};

use anvil_core::config::orchestrator::OrchestratorConfig;
use anvil_core::config::{AdapterTarget, AnvilConfig};
use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::antigravity::config::{ensure_object_field, read_json_object};
use crate::claude_code::mcp_servers::tool_pattern;
use crate::sync::{should_sync_codex, should_sync_cursor, should_sync_windsurf, write_if_changed};
use crate::{antigravity, claude_code, codex, cursor, windsurf};

/// One server Anvil manages: how it starts and how to recognize an entry Anvil wrote.
pub(crate) struct ManagedServer {
    /// Key under `mcpServers` (and the Codex section name); it also names the tools
    /// (`mcp__<name>__*`).
    pub name: &'static str,
    pub command: String,
    pub args: Vec<String>,
    /// `true` when `args` are the arguments Anvil writes for this server (any version or path).
    pub recognizes: fn(&[&str]) -> bool,
}

impl ManagedServer {
    /// The `mcpServers` entry JSON clients read.
    pub fn entry(&self) -> Value {
        json!({ "command": self.command, "args": self.args })
    }

    pub(crate) fn is_managed_entry(&self, entry: &Value) -> bool {
        let args = entry.get("args").and_then(Value::as_array);
        let args: Vec<&str> =
            args.map_or_else(Vec::new, |a| a.iter().filter_map(Value::as_str).collect());
        (self.recognizes)(&args)
    }

    /// The executor allowlist entry that grants every tool of the server.
    pub fn executor_tool(&self) -> String {
        tool_pattern(self.name)
    }
}

/// Adds or removes the managed server in a JSON file with an `mcpServers` object.
/// Returns whether the file changed.
pub(crate) fn merge_json_servers(
    path: &Path,
    label: &str,
    server: &ManagedServer,
    enabled: bool,
) -> Result<bool> {
    if !enabled && !path.exists() {
        return Ok(false);
    }
    let mut root = read_json_object(path)?;
    let servers = ensure_object_field(&mut root, "mcpServers")?;
    if servers.get(server.name).is_some_and(|entry| !server.is_managed_entry(entry)) {
        return Ok(false);
    }
    if enabled {
        servers.insert(server.name.to_owned(), server.entry());
    } else if servers.remove(server.name).is_none() {
        return Ok(false);
    }
    let formatted = serde_json::to_string_pretty(&Value::Object(root))
        .with_context(|| format!("failed to serialize {label}"))?;
    write_if_changed(path, &format!("{formatted}\n"))
}

/// Grants or revokes `pattern` for executors. Returns whether the list changed.
pub(crate) fn set_executor_tool(
    orchestrator: &mut OrchestratorConfig,
    pattern: &str,
    granted: bool,
) -> bool {
    let present = orchestrator.executor_tools.iter().any(|tool| tool == pattern);
    if granted && !present {
        orchestrator.executor_tools.push(pattern.to_owned());
    } else if !granted && present {
        orchestrator.executor_tools.retain(|tool| tool != pattern);
    } else {
        return false;
    }
    true
}

/// Brings every adapter's MCP config and the executor allowlist in line with `enabled`.
/// Returns the files that changed.
pub(crate) fn apply(
    repo_root: &Path,
    target: AdapterTarget,
    config: &mut AnvilConfig,
    server: &ManagedServer,
    enabled: bool,
) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for (path, label) in json_targets(repo_root, target) {
        if merge_json_servers(&path, label, server, enabled)? {
            written.push(path);
        }
    }
    if should_sync_codex(repo_root, target)
        && codex_toml::merge_codex_server(&codex::config_path(repo_root), server, enabled)?
    {
        written.push(codex::config_path(repo_root));
    }
    written.extend(sync_executor_grant(repo_root, config, target, server, enabled)?);
    Ok(written)
}

/// The JSON MCP config files of the adapters in play, with their display names.
fn json_targets(repo_root: &Path, target: AdapterTarget) -> Vec<(PathBuf, &'static str)> {
    let mut targets = Vec::new();
    if target.includes_claude() {
        targets.push((claude_code::config::mcp_config_path(repo_root), ".mcp.json"));
    }
    if target.includes_antigravity() {
        targets.push((antigravity::config::mcp_config_path(repo_root), ".agents/mcp_config.json"));
    }
    if should_sync_cursor(repo_root, target) {
        targets.push((cursor::mcp_config_path(repo_root), ".cursor/mcp.json"));
    }
    if should_sync_windsurf(repo_root, target) {
        targets.push((windsurf::mcp_config_path(repo_root), ".windsurf/mcp_config.json"));
    }
    targets
}

/// Updates `orchestrator.executor_tools` and regenerates the agents when it changed. A server
/// the user configured by hand keeps its grant when Anvil stops managing it.
fn sync_executor_grant(
    repo_root: &Path,
    config: &mut AnvilConfig,
    target: AdapterTarget,
    server: &ManagedServer,
    enabled: bool,
) -> Result<Vec<PathBuf>> {
    let granted = enabled || user_server_remains(repo_root, server.name);
    let Some(orchestrator) = config.orchestrator.as_mut() else {
        return Ok(Vec::new());
    };
    if !set_executor_tool(orchestrator, &server.executor_tool(), granted) {
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

/// `true` when some repository MCP config still defines a server called `name`.
fn user_server_remains(repo_root: &Path, name: &str) -> bool {
    let files = [
        claude_code::config::mcp_config_path(repo_root),
        antigravity::config::mcp_config_path(repo_root),
        cursor::mcp_config_path(repo_root),
        windsurf::mcp_config_path(repo_root),
    ];
    files.iter().any(|path| {
        read_json_object(path)
            .ok()
            .is_some_and(|root| root.get("mcpServers").and_then(|s| s.get(name)).is_some())
    }) || codex_toml::codex_server_present(&codex::config_path(repo_root), name)
}
