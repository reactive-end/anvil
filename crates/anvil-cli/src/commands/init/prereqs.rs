//! `anvil init` step that records what a machine needs for the project (non-interactive).

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anyhow::Result;

use crate::commands::prereqs::detect_set;

/// Detects the prerequisites of `dir`, stores them in the config, and returns the summary line.
pub(super) fn detect_and_store(dir: &Path, config: &mut AnvilConfig) -> Result<String> {
    let (_projects, set) = detect_set(dir, config)?;
    let line = format!(
        "{} tools, {} env files, {} MCP servers detected; run `anvil prereqs check`",
        set.tools.len(),
        set.env_files.len(),
        set.mcp_servers.len()
    );
    config.prerequisites.detected = set;
    config.save(dir)?;
    Ok(line)
}
