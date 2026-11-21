//! Fast Git CLI helpers for repository root, branch, HEAD commit, and changed diffs.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use rustc_hash::FxHashSet;

/// Scope of changed files to query from Git.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffScope {
    Staged,
    Unstaged,
    WorkingTree,
    Head,
}

/// Resolves the top-level Git repository root containing `start_dir`.
pub fn repo_root(start_dir: &Path) -> Result<PathBuf> {
    let out = run_git(start_dir, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(out.trim()))
}

/// Returns the active branch name (or `HEAD` when detached).
pub fn current_branch(repo_root: &Path) -> Result<String> {
    let out = run_git(repo_root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    Ok(out.trim().to_owned())
}

/// Returns the full SHA-1/SHA-256 commit hash of `HEAD`.
pub fn head_commit(repo_root: &Path) -> Result<String> {
    let out = run_git(repo_root, &["rev-parse", "HEAD"])?;
    Ok(out.trim().to_owned())
}

/// Lists unique relative paths changed under `scope`, normalized with forward slashes.
pub fn changed_files(repo_root: &Path, scope: DiffScope) -> Result<Vec<String>> {
    let mut unique = FxHashSet::default();
    match scope {
        DiffScope::Staged => {
            collect_lines(&run_git(repo_root, &["diff", "--cached", "--name-only"])?, &mut unique);
        }
        DiffScope::Unstaged => {
            collect_lines(&run_git(repo_root, &["diff", "--name-only"])?, &mut unique);
        }
        DiffScope::WorkingTree => {
            collect_lines(&run_git(repo_root, &["diff", "--cached", "--name-only"])?, &mut unique);
            collect_lines(&run_git(repo_root, &["diff", "--name-only"])?, &mut unique);
            collect_lines(
                &run_git(repo_root, &["ls-files", "--others", "--exclude-standard"])?,
                &mut unique,
            );
        }
        DiffScope::Head => {
            collect_lines(&run_git(repo_root, &["diff", "--name-only", "HEAD"])?, &mut unique);
            collect_lines(
                &run_git(repo_root, &["ls-files", "--others", "--exclude-standard"])?,
                &mut unique,
            );
        }
    }
    let mut files: Vec<String> = unique.into_iter().collect();
    files.sort();
    Ok(files)
}

/// Returns the unified diff for a specific file (`staged` selects `--cached`).
pub fn file_diff(repo_root: &Path, file_path: &str, staged: bool) -> Result<String> {
    if staged {
        run_git(repo_root, &["diff", "--cached", "--", file_path])
    } else {
        run_git(repo_root, &["diff", "--", file_path])
    }
}

/// Returns the full unified diff across the repository.
pub fn all_diff(repo_root: &Path, staged: bool) -> Result<String> {
    if staged {
        run_git(repo_root, &["diff", "--cached"])
    } else {
        run_git(repo_root, &["diff"])
    }
}

fn collect_lines(stdout: &str, out: &mut FxHashSet<String>) {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            out.insert(trimmed.replace('\\', "/"));
        }
    }
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to execute git {}", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
