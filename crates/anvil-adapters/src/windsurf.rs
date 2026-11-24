//! Non-destructive adapter generator for `.windsurf/mcp_config.json` and `.windsurf/rules/anvil.md`.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::antigravity::config::merge_mcp_servers_json;
use crate::sync::sync_managed_file;
use crate::templates;

/// Returns the path to `.windsurf/mcp_config.json`.
pub fn mcp_config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".windsurf").join("mcp_config.json")
}

/// Returns the path to `.windsurf/rules/anvil.md`.
pub fn rules_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".windsurf").join("rules").join("anvil.md")
}

/// Non-destructively merges the `anvil mcp` server entry into `.windsurf/mcp_config.json`.
pub fn merge_mcp_config(repo_root: &Path) -> Result<bool> {
    merge_mcp_servers_json(&mcp_config_path(repo_root), ".windsurf/mcp_config.json")
}

/// Writes or updates `.windsurf/rules/anvil.md`, preserving user text outside managed markers.
pub fn sync_rules(repo_root: &Path) -> Result<bool> {
    sync_managed_file(&rules_path(repo_root), templates::AGENTS_MD)
}
