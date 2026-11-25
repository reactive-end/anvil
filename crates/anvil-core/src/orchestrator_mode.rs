//! Local orchestrator-mode switch and the file scopes delegated to executors.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::fs_util::{atomic_write, with_file_lock};
use crate::ratchet::glob_matches;

const LOCK_TIMEOUT_MS: u64 = 2_000;
const SCOPE_TTL_MINUTES: i64 = 120;

/// A file or glob the orchestrator handed to executors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ScopeGrant {
    session: String,
    path: String,
    granted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct ModeState {
    enabled: bool,
    scopes: Vec<ScopeGrant>,
}

/// Returns `true` when orchestrator mode is on for this working copy.
pub fn is_enabled(root: &Path) -> bool {
    read_state(root).enabled
}

/// Turns orchestrator mode on or off, dropping every delegated scope.
pub fn set_enabled(root: &Path, enabled: bool) -> Result<()> {
    update_state(root, |state| {
        state.enabled = enabled;
        state.scopes.clear();
    })
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
            state
                .scopes
                .retain(|g| !(g.session == session && &g.path == path));
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
        let paths = parse_scope("Goal: x\n- SCOPE: `src/a.rs`, ./src/ui/, docs/*.md\nDone").unwrap();
        assert_eq!(paths, vec!["src/a.rs", "src/ui", "docs/*.md"]);
        for bad in ["no scope", "SCOPE:", "SCOPE: .", "SCOPE: **/*.rs", "SCOPE: /etc", "SCOPE: ../x", "SCOPE: C:/x"] {
            assert!(parse_scope(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn scope_matches_files_directories_and_globs() {
        let scope = vec!["src/a.rs".to_owned(), "src/ui".to_owned(), "docs/*.md".to_owned(), "*.toml".to_owned()];
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
}
