//! Before work starts, make sure the machine has what the repository needs: the detected
//! prerequisites, a local SonarQube where Sonar is enabled, and MCP Toolbox for projects that use
//! a database. `SessionStart` tells the agent what is missing and how to fix it; `anvil prereqs
//! check` and `anvil doctor` list the same items.

mod database;
mod sonar;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anvil_adapters::contract::{InboxMessage, MessageLevel};
use anvil_core::config::AnvilConfig;
use anvil_prereqs::{Item, Project, Report};
use serde::{Deserialize, Serialize};

/// How long a session-start result is reused before the machine is probed again.
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);

/// The Sonar and database items for `projects`, on top of the detected prerequisites.
pub fn extra_items(root: &Path, config: &AnvilConfig, projects: &[Project]) -> Vec<Item> {
    let mut items = sonar::items(root, config);
    items.extend(database::items(root, config, projects));
    items
}

/// `report` with the Sonar and database items added (restricted to `subrepo` when given).
pub fn with_extras(
    root: &Path,
    config: &AnvilConfig,
    projects: &[Project],
    report: Report,
    subrepo: Option<&str>,
) -> Report {
    let extras = extra_items(root, config, projects)
        .into_iter()
        .filter(|item| subrepo.is_none_or(|name| item.subrepo.as_deref() == Some(name)));
    Report::new(report.items.into_iter().chain(extras).collect())
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cached {
    checked_at: u64,
    config_mtime: u64,
    failures: Vec<String>,
}

/// The session-start warning: one line per required item that is missing or at the wrong
/// version, with its fix. `None` when the machine is ready. Results are cached for ten minutes
/// (or until `.harness/config.json` changes) so starting a session stays fast.
pub fn session_message(root: &Path, config: &AnvilConfig) -> Option<InboxMessage> {
    let failures = cached_failures(root).unwrap_or_else(|| {
        let failures = probe_failures(root, config);
        store(root, &failures);
        failures
    });
    if failures.is_empty() {
        return None;
    }
    let text = format!(
        "This repository needs tools that are missing on this machine. Fix them (or tell the user exactly what to install) before changing code; `anvil prereqs check` re-checks.\n- {}",
        failures.join("\n- ")
    );
    Some(InboxMessage::new("anvil-readiness", MessageLevel::Warning, text, None))
}

fn probe_failures(root: &Path, config: &AnvilConfig) -> Vec<String> {
    match crate::commands::prereqs::run_check(root, config, None) {
        Ok(report) => report.items.iter().filter(|i| i.is_failure()).map(describe).collect(),
        Err(err) => vec![format!("the prerequisite check failed: {err:#}")],
    }
}

fn describe(item: &Item) -> String {
    let place = item.subrepo.as_deref().map(|s| format!(" ({s})")).unwrap_or_default();
    let why = item.message.as_deref().map(|m| format!(": {m}")).unwrap_or_default();
    let fix = item.hint.as_deref().map(|h| format!(" -> {h}")).unwrap_or_default();
    format!("{}{place}{why}{fix}", item.name)
}

fn cache_path(root: &Path) -> PathBuf {
    root.join(".harness/.state/readiness.json")
}

fn config_mtime(root: &Path) -> u64 {
    fs::metadata(AnvilConfig::path(root))
        .and_then(|m| m.modified())
        .map_or(0, |t| t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn cached_failures(root: &Path) -> Option<Vec<String>> {
    let raw = fs::read(cache_path(root)).ok()?;
    let cached: Cached = serde_json::from_slice(&raw).ok()?;
    let fresh = now().saturating_sub(cached.checked_at) < CACHE_TTL.as_secs()
        && cached.config_mtime == config_mtime(root);
    fresh.then_some(cached.failures)
}

/// Best effort: a cache that cannot be written only means the next session probes again.
fn store(root: &Path, failures: &[String]) {
    let cached =
        Cached { checked_at: now(), config_mtime: config_mtime(root), failures: failures.to_vec() };
    if let Ok(json) = serde_json::to_vec_pretty(&cached) {
        let path = cache_path(root);
        let written =
            path.parent().map_or(Ok(()), fs::create_dir_all).and_then(|()| fs::write(&path, json));
        if let Err(err) = written {
            crate::commands::hook::error_log::log_hook_error(
                root,
                "readiness:cache",
                &err.to_string(),
            );
        }
    }
}
