//! Non-destructive Git `pre-commit` hook installer for human commits (`anvil check --staged --tier 0`).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use anvil_core::fs_util::atomic_write;

const HOOK_START: &str = "# anvil:pre-commit:start";
const HOOK_END: &str = "# anvil:pre-commit:end";
const MANAGED_SNIPPET: &str = "# anvil:pre-commit:start\nif command -v anvil >/dev/null 2>&1; then\n  anvil check --staged --tier 0\nfi\n# anvil:pre-commit:end";

/// Installs or refreshes Anvil's `pre-commit` snippet inside `.git/hooks/pre-commit`.
/// Returns `Ok(true)` if the hook file changed, or `Ok(false)` if no `.git` dir or already current.
pub fn install_pre_commit_hook(repo_root: &Path) -> Result<bool> {
    let git_dir = repo_root.join(".git");
    if !git_dir.is_dir() {
        return Ok(false);
    }
    let hook_path = git_dir.join("hooks").join("pre-commit");
    let existing = fs::read_to_string(&hook_path).unwrap_or_default();
    let updated = merge_hook_content(&existing);
    if existing == updated {
        return Ok(false);
    }

    atomic_write(&hook_path, updated.as_bytes())
        .with_context(|| format!("failed to write {}", hook_path.display()))?;
    set_executable_if_unix(&hook_path)?;
    Ok(true)
}

fn merge_hook_content(existing: &str) -> String {
    if existing.trim().is_empty() {
        return format!("#!/bin/sh\n{MANAGED_SNIPPET}\n");
    }
    if let Some(start) = existing.find(HOOK_START)
        && let Some(rel_end) = existing[start..].find(HOOK_END)
    {
        let end = start + rel_end + HOOK_END.len();
        let before = existing[..start].trim_end_matches(['\r', '\n']);
        let after = existing[end..].trim_start_matches(['\r', '\n']);
        return match (before.is_empty(), after.is_empty()) {
            (true, true) => format!("{MANAGED_SNIPPET}\n"),
            (true, false) => format!("{MANAGED_SNIPPET}\n\n{after}\n"),
            (false, true) => format!("{before}\n\n{MANAGED_SNIPPET}\n"),
            (false, false) => format!("{before}\n\n{MANAGED_SNIPPET}\n\n{after}\n"),
        };
    }
    let body = existing.trim_end_matches(['\r', '\n']);
    format!("{body}\n\n{MANAGED_SNIPPET}\n")
}

#[cfg(unix)]
fn set_executable_if_unix(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable_if_unix(_path: &Path) -> Result<()> {
    Ok(())
}
