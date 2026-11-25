//! Orchestrator/executor agent definitions for Claude Code.
//!
//! Claude Code's model picker rows hold a single model, so the pair lives in two agent files whose
//! frontmatter carries a model and an effort each.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use anvil_core::config::orchestrator::OrchestratorConfig;
use serde_json::Value;

use crate::antigravity::config::read_json_object;
use crate::sync::write_if_changed;
use crate::templates;

/// Name of the agent that plans, decides, and reviews.
pub const ORCHESTRATOR_AGENT: &str = "anvil-orchestrator";

/// Name of the agent that writes code within a delegated scope.
pub const EXECUTOR_AGENT: &str = "anvil-executor";

const MANAGED_MARKER: &str = "<!-- anvil:managed:agent -->";

/// Writes the orchestrator and executor agents plus the `/orchestrate` skill, returning changed files.
///
/// Files that exist without the managed marker belong to the user and are left untouched.
pub fn sync_agents(repo_root: &Path, config: &OrchestratorConfig) -> Result<Vec<PathBuf>> {
    let claude = repo_root.join(".claude");
    let targets = [
        (
            claude.join("agents").join("anvil-orchestrator.md"),
            templates::ORCHESTRATOR_AGENT_MD,
        ),
        (
            claude.join("agents").join("anvil-executor.md"),
            templates::EXECUTOR_AGENT_MD,
        ),
        (
            claude.join("skills").join("orchestrate").join("SKILL.md"),
            templates::ORCHESTRATE_SKILL_MD,
        ),
    ];
    let mut written = Vec::new();
    for (path, template) in targets {
        let is_user_owned =
            fs::read_to_string(&path).is_ok_and(|existing| !existing.contains(MANAGED_MARKER));
        if !is_user_owned && write_if_changed(&path, &render(template, config))? {
            written.push(path);
        }
    }
    Ok(written)
}

/// Returns the path to the per-user `.claude/settings.local.json`.
pub fn local_settings_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".claude").join("settings.local.json")
}

/// Returns `true` when new sessions in this repository start as the orchestrator.
pub fn is_default_agent(repo_root: &Path) -> bool {
    read_json_object(&local_settings_path(repo_root)).is_ok_and(|settings| {
        settings.get("agent").and_then(Value::as_str) == Some(ORCHESTRATOR_AGENT)
    })
}

/// Makes the orchestrator the default agent of new sessions, or stops doing so.
///
/// Refuses to replace a different `agent` the user configured.
pub fn set_default_agent(repo_root: &Path, enabled: bool) -> Result<bool> {
    let path = local_settings_path(repo_root);
    let mut settings = read_json_object(&path)?;
    let current = settings.get("agent").and_then(Value::as_str);
    if enabled {
        if let Some(other) = current.filter(|name| *name != ORCHESTRATOR_AGENT) {
            bail!("`{}` already sets `agent` to `{other}`", path.display());
        }
        settings.insert("agent".to_owned(), Value::String(ORCHESTRATOR_AGENT.to_owned()));
    } else if current == Some(ORCHESTRATOR_AGENT) {
        settings.remove("agent");
    } else {
        return Ok(false);
    }
    let json = serde_json::to_string_pretty(&Value::Object(settings))
        .context("failed to serialize .claude/settings.local.json")?;
    write_if_changed(&path, &format!("{json}\n"))
}

fn render(template: &str, config: &OrchestratorConfig) -> String {
    template
        .replace("\r\n", "\n")
        .replace("{{orchestrator_model}}", &config.orchestrator.model)
        .replace("{{orchestrator_effort}}", config.orchestrator.effort.as_str())
        .replace("{{executor_model}}", &config.executor.model)
        .replace("{{executor_effort}}", config.executor.effort.as_str())
}
