//! Resolves which project files belong to a task instead of attributing the whole dirty tree.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::SystemTime;

use anvil_core::git::{self, DiffScope};

use crate::active::load_active_task;
use crate::builder::read_single_task;
use crate::curate::{load_first_untracked_edit_at, load_untracked_edits};

/// Returns `true` for repository-relative project paths (excludes harness state and paths
/// outside the repository, which would leak machine-specific locations into the shared ledger).
pub fn is_project_file(path: &str) -> bool {
    let norm = path.trim().replace('\\', "/");
    let is_outside = norm.starts_with('/')
        || norm == ".."
        || norm.starts_with("../")
        || norm.as_bytes().get(1) == Some(&b':');
    !norm.is_empty() && !is_outside && !norm.starts_with(".harness/") && norm != "tasks.json"
}

/// Returns `true` when git ignores `rel` (a path relative to `repo`); tracked files never are.
pub fn is_git_ignored(repo: &Path, rel: &str) -> bool {
    without_ignored(repo, vec![rel.to_owned()]).is_empty()
}

/// Drops the paths git ignores: edits there are scratch work, not part of any task. When `repo` is
/// not a git repository, or git cannot be run, every path is kept.
pub fn without_ignored(repo: &Path, files: Vec<String>) -> Vec<String> {
    let Some(ignored) = ignored_among(repo, &files) else {
        return files;
    };
    files.into_iter().filter(|file| !ignored.contains(file)).collect()
}

fn ignored_among(repo: &Path, files: &[String]) -> Option<BTreeSet<String>> {
    if files.is_empty() {
        return None;
    }
    let mut child = Command::new("git")
        .args(["check-ignore", "-z", "--stdin"])
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let input: String = files.iter().map(|file| format!("{file}\0")).collect();
    stdin.write_all(input.as_bytes()).ok()?;
    drop(stdin);
    let out = child.wait_with_output().ok()?;
    // Exit 0 lists ignored paths, 1 means none are; anything else (not a repository) is no answer.
    matches!(out.status.code(), Some(0 | 1)).then(|| {
        String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect()
    })
}

/// The files a review of `task_id` covers: those its entries recorded plus its pending edits.
///
/// Packets, approvals, and the close check all use this one set, so they agree on what was reviewed.
pub fn review_files(repo: &Path, task_id: &str) -> Vec<String> {
    let mut set: BTreeSet<String> = read_single_task(repo, task_id)
        .map(|task| task.files_touched)
        .unwrap_or_default()
        .into_iter()
        .collect();
    set.extend(collect_task_files(repo, Some(task_id), &[]));
    set.into_iter().filter(|f| is_project_file(f)).collect()
}

/// Collects files for a ledger entry: explicit paths win, otherwise session edits plus dirty
/// files modified since the work began. Dirty files that predate the task are left out.
pub fn collect_task_files(repo: &Path, task_id: Option<&str>, explicit: &[String]) -> Vec<String> {
    if !explicit.is_empty() {
        return explicit.to_vec();
    }
    let mut set: BTreeSet<String> = load_untracked_edits(repo).into_iter().collect();
    let mut since = load_first_untracked_edit_at(repo);
    if let Ok(Some(active)) = load_active_task(repo)
        && task_id.is_some_and(|id| id == active.task_id)
    {
        set.extend(active.touched_files);
        since = Some(since.map_or(active.started_at, |s| s.min(active.started_at)));
    }
    let baseline = since.map(SystemTime::from);
    for rel in git::changed_files(repo, DiffScope::WorkingTree).unwrap_or_default() {
        if baseline.is_none_or(|base| changed_since(&repo.join(&rel), base)) {
            set.insert(rel.replace('\\', "/"));
        }
    }
    without_ignored(repo, set.into_iter().filter(|f| is_project_file(f)).collect())
}

/// Deleted files have no mtime, so their parent directory is consulted instead.
fn changed_since(path: &Path, baseline: SystemTime) -> bool {
    let probe = if path.exists() { Some(path) } else { path.parent() };
    probe
        .and_then(|p| fs::metadata(p).ok())
        .and_then(|meta| meta.modified().ok())
        .is_some_and(|mtime| mtime >= baseline)
}

#[cfg(test)]
mod tests;
