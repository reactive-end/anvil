//! Cached team identity resolution (`gh api user` -> `git config user.name` -> `"unknown"`).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration as StdDuration, Instant};

use anyhow::Result;
use chrono::{Duration, Utc};

pub use anvil_core::fs_util::sanitize_author;

use crate::model::IdentityCache;
use crate::{atomic_write_json, read_json_file};

const CACHE_TTL_HOURS: i64 = 24;
const GH_CMD_TIMEOUT_MS: u64 = 2000;

/// Returns the path to `.harness/.state/identity.json`.
pub fn identity_cache_path(repo: &Path) -> PathBuf {
    repo.join(".harness").join(".state").join("identity.json")
}

/// Resolves the active author, consulting `.harness/.state/identity.json` first.
///
/// Resolution order on cache miss: `gh api user --jq .login` (with 2s timeout),
/// then `git config user.name`, then `"unknown"`. Resolved identities (other than
/// `"unknown"`) are cached atomically.
pub fn resolve_author(repo: &Path) -> Result<String> {
    if let Some(cached) = read_cached_author(repo, Some(CACHE_TTL_HOURS)) {
        return Ok(cached);
    }

    if let Some(gh_user) = run_identity_cmd_with_timeout(
        repo,
        "gh",
        &["api", "user", "--jq", ".login"],
        GH_CMD_TIMEOUT_MS,
    ) {
        let sanitized = sanitize_author(&gh_user);
        if sanitized != "unknown" {
            let _ = write_identity_cache(repo, &sanitized, "gh");
            return Ok(sanitized);
        }
    }

    if let Some(git_user) = resolve_git_user_name(repo) {
        let _ = write_identity_cache(repo, &git_user, "git");
        return Ok(git_user);
    }

    Ok("unknown".to_owned())
}

/// Fast, offline author resolution for hooks with ZERO live `gh api user` network calls.
///
/// Resolution order: `.harness/.state/identity.json` cache (even if older than 24h)
/// -> `git config user.name` -> `"unknown"`.
pub fn resolve_author_fast(repo_root: &Path) -> Result<String> {
    if let Some(cached) = read_cached_author(repo_root, None) {
        return Ok(cached);
    }
    if let Some(git_user) = resolve_git_user_name(repo_root) {
        return Ok(git_user);
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

fn read_cached_author(repo: &Path, max_age_hours: Option<i64>) -> Option<String> {
    let path = identity_cache_path(repo);
    if !path.exists() {
        return None;
    }
    let cache: IdentityCache = read_json_file(&path).ok()?;
    if let Some(ttl_hours) = max_age_hours {
        let age = Utc::now() - cache.resolved_at;
        if age > Duration::hours(ttl_hours) {
            return None;
        }
    }
    let clean = sanitize_author(&cache.author);
    (clean != "unknown").then_some(clean)
}

fn resolve_git_user_name(repo: &Path) -> Option<String> {
    let raw = run_identity_cmd(repo, "git", &["config", "user.name"])?;
    let sanitized = sanitize_author(&raw);
    (sanitized != "unknown").then_some(sanitized)
}

fn run_identity_cmd(repo: &Path, bin: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(bin).args(args).current_dir(repo).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn run_identity_cmd_with_timeout(
    repo: &Path,
    bin: &str,
    args: &[&str],
    timeout_ms: u64,
) -> Option<String> {
    let mut child = Command::new(bin)
        .args(args)
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + StdDuration::from_millis(timeout_ms);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let output = child.wait_with_output().ok()?;
                let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                return (!text.is_empty()).then_some(text);
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                thread::sleep(StdDuration::from_millis(20));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}
