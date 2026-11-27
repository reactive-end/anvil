//! The few git queries documentation freshness needs.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use anvil_core::git::working_tree_status;

fn run(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").arg("-C").arg(root).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Abbreviated `HEAD` commit, or `None` outside a repository or before the first commit.
pub fn head_short(root: &Path) -> Option<String> {
    let out = run(root, &["rev-parse", "--short=12", "HEAD"])?;
    Some(out.trim().to_owned()).filter(|sha| !sha.is_empty())
}

/// `true` when `commit` names a commit of the repository.
pub fn commit_exists(root: &Path, commit: &str) -> bool {
    if commit.is_empty() || commit.starts_with('-') {
        return false;
    }
    run(root, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_some()
}

/// Repo-relative files that differ from `commit`: committed changes since it, plus uncommitted and
/// untracked changes in the working tree. `None` when `commit` is unknown.
pub fn changed_since(root: &Path, commit: &str) -> Option<BTreeSet<String>> {
    if !commit_exists(root, commit) {
        return None;
    }
    let listed = run(root, &["-c", "core.quotepath=off", "diff", "--name-only", "-z", commit])?;
    let mut files: BTreeSet<String> =
        listed.split('\0').filter(|p| !p.is_empty()).map(|p| p.replace('\\', "/")).collect();
    if let Ok(status) = working_tree_status(root) {
        files.extend(
            status
                .added
                .into_iter()
                .chain(status.modified)
                .chain(status.deleted)
                .chain(status.untracked),
        );
    }
    files.retain(|f| !f.starts_with(".harness/docs/") && !f.starts_with(".harness/.state/"));
    Some(files)
}
