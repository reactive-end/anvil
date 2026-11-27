//! Non-destructive adapter generator for `.cursor/mcp.json` and `.cursor/rules/anvil.mdc`.

use std::path::{Path, PathBuf};

use anvil_core::config::AnvilConfig;
use anyhow::Result;

use crate::antigravity::config::merge_mcp_servers_json;
use crate::architecture;
use crate::sync::{merge_managed_markdown, read_optional_string, write_if_changed};
use crate::templates;

const CURSOR_RULES_FRONTMATTER: &str = "\
---
description: Anvil deterministic harness discipline
globs:
alwaysApply: true
---

";

/// Returns the path to `.cursor/mcp.json`.
pub fn mcp_config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".cursor").join("mcp.json")
}

/// Returns the path to `.cursor/rules/anvil.mdc`.
pub fn rules_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".cursor").join("rules").join("anvil.mdc")
}

/// Non-destructively merges the `anvil mcp` server entry into `.cursor/mcp.json`.
pub fn merge_mcp_config(repo_root: &Path) -> Result<bool> {
    merge_mcp_servers_json(&mcp_config_path(repo_root), ".cursor/mcp.json")
}

/// Non-destructively merges per-subrepo `anvil-<slug>` MCP entries into `.cursor/mcp.json`.
pub fn merge_umbrella_mcp_config(root: &Path, subrepos: &[(String, PathBuf)]) -> Result<bool> {
    crate::antigravity::config::merge_umbrella_mcp_servers_json(
        &mcp_config_path(root),
        ".cursor/mcp.json",
        subrepos,
    )
}

/// Writes or updates `.cursor/rules/anvil.mdc`, preserving user text outside managed markers.
pub fn sync_rules(repo_root: &Path) -> Result<bool> {
    let config = AnvilConfig::load_or_default(repo_root)?;
    let template = architecture::with_architecture(templates::CURSOR_RULES_MD, &config);
    sync_rules_with_template(repo_root, &template)
}

/// Writes or updates `.cursor/rules/anvil.mdc` with an explicit managed markdown template.
pub fn sync_rules_with_template(repo_root: &Path, template: &str) -> Result<bool> {
    let path = rules_path(repo_root);
    let existing = read_optional_string(&path)?;
    let merged = match existing.as_deref() {
        Some(current) if !current.trim().is_empty() => {
            merge_managed_markdown(Some(current), template)
        }
        _ => format!("{CURSOR_RULES_FRONTMATTER}{template}"),
    };
    write_if_changed(&path, &merged)
}
