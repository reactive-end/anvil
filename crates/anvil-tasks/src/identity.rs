//! Cached team identity resolution (`gh api user` -> `git config user.name` -> `"unknown"`).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use chrono::{Duration, Utc};

use crate::fs_util::{atomic_write_json, read_json_file};
use crate::model::IdentityCache;

const CACHE_TTL_HOURS: i64 = 24;

/// Returns the path to `.harness/.state/identity.json`.
pub fn identity_cache_path(repo: &Path) -> PathBuf {
    repo.join(".harness").join(".state").join("identity.json")
}

/// Resolves the active author, consulting `.harness/.state/identity.json` first.
///
/// Resolution order on cache miss: `gh api user --jq .login`, then `git config user.name`,
/// then `"unknown"`. Resolved identities (other than `"unknown"`) are cached atomically.
pub fn resolve_author(repo: &Path) -> Result<String> {
    if let Some(cached) = read_fresh_cache(repo) {
        return Ok(cached);
    }

    if let Some(gh_user) = run_identity_cmd(repo, "gh", &["api", "user", "--jq", ".login"]) {
        let sanitized = sanitize_author(&gh_user);
        if sanitized != "unknown" {
            let _ = write_identity_cache(repo, &sanitized, "gh");
            return Ok(sanitized);
        }
    }

    if let Some(git_user) = run_identity_cmd(repo, "git", &["config", "user.name"]) {
        let sanitized = sanitize_author(&git_user);
        if sanitized != "unknown" {
            let _ = write_identity_cache(repo, &sanitized, "git");
            return Ok(sanitized);
        }
    }

    Ok("unknown".to_owned())
}

/// Persists an explicit author identity into `.harness/.state/identity.json`.
pub fn write_identity_cache(repo: &Path, author: &str, source: &str) -> Result<()> {
    let record = IdentityCache {
        author: sanitize_author(author),
        source: source.to_owned(),
        resolved_at: Utc::now(),
    };
    atomic_write_json(&identity_cache_path(repo), &record)
}

/// Sanitizes an author string so it is safe in filenames such as `<author>-<ulid>.json`.
pub fn sanitize_author(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_dash = false;

    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }

    while out.ends_with('-') {
        out.pop();
    }

    if out.is_empty() {
        "unknown".to_owned()
    } else {
        out
    }
}

fn read_fresh_cache(repo: &Path) -> Option<String> {
    let path = identity_cache_path(repo);
    if !path.exists() {
        return None;
    }
    let cache: IdentityCache = read_json_file(&path).ok()?;
    let age = Utc::now() - cache.resolved_at;
    if age > Duration::hours(CACHE_TTL_HOURS) {
        return None;
    }
    let clean = sanitize_author(&cache.author);
    (clean != "unknown").then_some(clean)
}

fn run_identity_cmd(repo: &Path, bin: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(bin).args(args).current_dir(repo).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}
