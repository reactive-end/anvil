//! Local orchestrator-mode switch and the file scopes delegated to executors.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::fs_util::{atomic_write, with_file_lock};
use crate::ratchet::glob_matches;

mod overlap;

pub use overlap::overlapping_entry;

const LOCK_TIMEOUT_MS: u64 = 2_000;
const SCOPE_TTL_MINUTES: i64 = 120;
/// How long a started executor counts as starting before its own transcript shows up.
const SPAWN_TTL_MINUTES: i64 = 10;

/// A file or glob the orchestrator handed to executors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ScopeGrant {
    session: String,
    path: String,
    granted_at: DateTime<Utc>,
}

/// An executor the orchestrator started, keyed by the id of the `Agent` call that started it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SpawnGrant {
    session: String,
    tool_use_id: String,
    paths: Vec<String>,
    granted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct ModeState {
    enabled: bool,
    /// The agent was last told the mode is on, so turning it off needs one explicit notice:
    /// otherwise earlier "delegate everything" instructions stay in its context.
    announced_on: bool,
    scopes: Vec<ScopeGrant>,
    spawns: Vec<SpawnGrant>,
}

/// What the next prompt should tell the agent about the mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Announcement {
    /// The mode is on: repeat the standing instructions.
    On,
    /// The mode was switched off since the agent was last told it was on: say so once.
    TurnedOff,
}

/// Returns `true` when orchestrator mode is on for this working copy.
pub fn is_enabled(root: &Path) -> bool {
    read_state(root).enabled
}

/// Decides the mode announcement for a new prompt and records it, so `TurnedOff` is given once.
/// `None` when the mode is off and the agent already knows.
pub fn next_announcement(root: &Path) -> Result<Option<Announcement>> {
    let state = read_state(root);
    let announcement = match (state.enabled, state.announced_on) {
        (true, _) => Some(Announcement::On),
        (false, true) => Some(Announcement::TurnedOff),
        (false, false) => None,
    };
    if state.announced_on != state.enabled {
        update_state(root, |state| state.announced_on = state.enabled)?;
    }
    Ok(announcement)
}

/// Turns orchestrator mode on or off, dropping every delegated scope.
pub fn set_enabled(root: &Path, enabled: bool) -> Result<()> {
    update_state(root, |state| {
        state.enabled = enabled;
        state.scopes.clear();
        state.spawns.clear();
    })
}

/// Remembers the scope of an executor started by the `Agent` call `tool_use_id`.
pub fn record_spawn(root: &Path, session: &str, tool_use_id: &str, paths: &[String]) -> Result<()> {
    update_state(root, |state| {
        let now = Utc::now();
        state.spawns.retain(|g| g.tool_use_id != tool_use_id && is_recent(g.granted_at, now));
        state.spawns.push(SpawnGrant {
            session: session.to_owned(),
            tool_use_id: tool_use_id.to_owned(),
            paths: paths.to_vec(),
            granted_at: now,
        });
    })
}

/// Executors started in `session` during the last few minutes, as `(tool_use_id, paths)`.
pub fn recent_spawns(root: &Path, session: &str) -> Vec<(String, Vec<String>)> {
    let now = Utc::now();
    read_state(root)
        .spawns
        .into_iter()
        .filter(|g| g.session == session && is_recent(g.granted_at, now))
        .map(|g| (g.tool_use_id, g.paths))
        .collect()
}

/// Extracts the paths of every `SCOPE:` line in a delegation brief.
///
/// Rejects whole-repository scopes: an executor that may touch anything has no scope at all.
pub fn parse_scope(brief: &str) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for line in brief.lines() {
        let bare = line.trim().trim_start_matches(['-', '*', '#', ' ']);
        let Some(rest) = bare.strip_prefix("SCOPE:") else {
            continue;
        };
        for raw in rest.split([',', ' ']) {
            let path = normalize(raw.trim_matches(['`', '"', '\'', '[', ']', ' ']));
            if !path.is_empty() {
                paths.push(path);
            }
        }
    }
    if paths.is_empty() {
        bail!("missing `SCOPE: <path>, <path>` line");
    }
    if let Some(wide) = paths.iter().find(|p| is_unbounded(p)) {
        bail!("scope `{wide}` is not limited to specific files or directories of this repository");
    }
    Ok(paths)
}

/// Adds delegated paths for `session`.
pub fn grant_scope(root: &Path, session: &str, paths: &[String]) -> Result<()> {
    update_state(root, |state| {
        let now = Utc::now();
        state.scopes.retain(|g| is_live(g, now));
        for path in paths {
            state.scopes.retain(|g| !(g.session == session && &g.path == path));
            state.scopes.push(ScopeGrant {
                session: session.to_owned(),
                path: path.clone(),
                granted_at: now,
            });
        }
    })
}

/// Returns the live paths delegated in `session`.
pub fn granted_scope(root: &Path, session: &str) -> Vec<String> {
    let now = Utc::now();
    read_state(root)
        .scopes
        .into_iter()
        .filter(|g| g.session == session && is_live(g, now))
        .map(|g| g.path)
        .collect()
}

/// Returns `true` when `rel_path` is one of, or inside, the delegated paths.
pub fn scope_allows(scope: &[String], rel_path: &str) -> bool {
    let rel = normalize(rel_path);
    scope.iter().any(|entry| {
        if entry.contains('*') {
            let same_level = entry.contains('/') || !rel.contains('/');
            return same_level && glob_matches(entry, &rel);
        }
        rel == *entry || rel.starts_with(&format!("{entry}/"))
    })
}

fn normalize(raw: &str) -> String {
    let unified = raw.replace('\\', "/");
    let trimmed = unified.strip_prefix("./").unwrap_or(&unified);
    trimmed.trim_end_matches('/').to_owned()
}

fn is_unbounded(path: &str) -> bool {
    let escapes = path.starts_with('/')
        || path.starts_with('~')
        || path.contains(':')
        || path.split('/').any(|seg| seg == "..");
    let first = path.split('/').next().unwrap_or(path);
    escapes || path == "." || first.chars().all(|c| c == '*')
}

fn is_recent(at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    now - at < Duration::minutes(SPAWN_TTL_MINUTES)
}

fn is_live(grant: &ScopeGrant, now: DateTime<Utc>) -> bool {
    now - grant.granted_at < Duration::minutes(SCOPE_TTL_MINUTES)
}

fn state_path(root: &Path) -> PathBuf {
    root.join(".harness").join(".state").join("orchestrator.json")
}

fn read_state(root: &Path) -> ModeState {
    fs::read(state_path(root))
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

fn update_state(root: &Path, change: impl FnOnce(&mut ModeState)) -> Result<()> {
    let path = state_path(root);
    with_file_lock(&path.with_extension("json.lock"), LOCK_TIMEOUT_MS, || {
        let mut state = read_state(root);
        change(&mut state);
        let json = serde_json::to_vec_pretty(&state).context("failed to serialize mode state")?;
        atomic_write(&path, &json)
    })
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn parses_bounded_scopes_and_rejects_unbounded_ones() {
        let paths =
            parse_scope("Goal: x\n- SCOPE: `src/a.rs`, ./src/ui/, docs/*.md\nDone").unwrap();
        assert_eq!(paths, vec!["src/a.rs", "src/ui", "docs/*.md"]);
        for bad in [
            "no scope",
            "SCOPE:",
            "SCOPE: .",
            "SCOPE: **/*.rs",
            "SCOPE: /etc",
            "SCOPE: ../x",
            "SCOPE: C:/x",
        ] {
            assert!(parse_scope(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn scope_matches_files_directories_and_globs() {
        let scope = vec![
            "src/a.rs".to_owned(),
            "src/ui".to_owned(),
            "docs/*.md".to_owned(),
            "*.toml".to_owned(),
        ];
        for ok in ["src/a.rs", "./src/ui/x/y.rs", "docs/a.md", "Cargo.toml"] {
            assert!(scope_allows(&scope, ok), "{ok}");
        }
        for bad in ["src/b.rs", "src/uix.rs", "docs/sub/a.md", "crates/x/Cargo.toml"] {
            assert!(!scope_allows(&scope, bad), "{bad}");
        }
    }

    #[test]
    fn scopes_belong_to_one_session_and_reset_with_the_mode() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".harness/.state")).unwrap();
        assert!(!is_enabled(root));
        set_enabled(root, true).unwrap();
        grant_scope(root, "s1", &["src/a.rs".to_owned()]).unwrap();
        assert!(is_enabled(root));
        assert_eq!(granted_scope(root, "s1"), vec!["src/a.rs"]);
        assert!(granted_scope(root, "s2").is_empty());
        set_enabled(root, false).unwrap();
        assert!(granted_scope(root, "s1").is_empty());
    }

    #[test]
    fn switching_the_mode_off_is_announced_once() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".harness/.state")).unwrap();
        assert_eq!(next_announcement(root).unwrap(), None);
        set_enabled(root, true).unwrap();
        assert_eq!(next_announcement(root).unwrap(), Some(Announcement::On));
        assert_eq!(next_announcement(root).unwrap(), Some(Announcement::On));
        set_enabled(root, false).unwrap();
        assert_eq!(next_announcement(root).unwrap(), Some(Announcement::TurnedOff));
        assert_eq!(next_announcement(root).unwrap(), None);
    }

    #[test]
    fn spawns_are_kept_per_session_and_replaced_by_call_id() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".harness/.state")).unwrap();
        record_spawn(root, "s1", "toolu_1", &["src/a".to_owned()]).unwrap();
        record_spawn(root, "s1", "toolu_1", &["src/b".to_owned()]).unwrap();
        record_spawn(root, "s2", "toolu_2", &["src/c".to_owned()]).unwrap();
        assert_eq!(
            recent_spawns(root, "s1"),
            vec![("toolu_1".to_owned(), vec!["src/b".to_owned()])]
        );
        set_enabled(root, false).unwrap();
        assert!(recent_spawns(root, "s2").is_empty());
    }
}
