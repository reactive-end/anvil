//! Fast Git helpers: pure-Rust repo root walk and batched/NUL-safe status & diff queries.

mod diff;

pub use diff::{batched_staged_diffs, batched_working_tree_diffs};

use std::fs;
use std::path::{Component, Path, PathBuf};
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

/// Categorized working tree paths from a single `git status --porcelain=v1 -z` call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkingTreeStatus {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    pub untracked: Vec<String>,
}

/// Resolves the top-level Git repository root containing `start_dir` via pure-Rust upward walk.
pub fn repo_root(start_dir: &Path) -> Result<PathBuf> {
    let abs = if start_dir.is_absolute() {
        start_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .context("failed to read current working directory")?
            .join(start_dir)
    };
    let normalized = normalize_lexical(&abs);
    let start = if normalized.is_file() {
        normalized.parent().map(Path::to_path_buf).unwrap_or(normalized)
    } else {
        normalized
    };

    for dir in start.ancestors() {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() || is_git_worktree_file(&dot_git) {
            return Ok(dir.to_path_buf());
        }
    }
    bail!("not inside a git repository: {}", start_dir.display())
}

fn is_git_worktree_file(dot_git: &Path) -> bool {
    dot_git.is_file()
        && fs::read_to_string(dot_git)
            .map(|s| s.trim_start().starts_with("gitdir:"))
            .unwrap_or(false)
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Returns the active branch name (using `symbolic-ref` fallback for unborn `HEAD`, or `HEAD` when detached).
pub fn current_branch(repo_root: &Path) -> Result<String> {
    if let Ok(sym) = run_git(repo_root, &["symbolic-ref", "--short", "HEAD"]) {
        let name = sym.trim();
        if !name.is_empty() {
            return Ok(name.to_owned());
        }
    }
    let out = run_git(repo_root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    Ok(out.trim().to_owned())
}

/// Returns the full SHA-1/SHA-256 commit hash of `HEAD`.
pub fn head_commit(repo_root: &Path) -> Result<String> {
    let out = run_git(repo_root, &["rev-parse", "HEAD"])?;
    Ok(out.trim().to_owned())
}

/// Inspects added, modified, deleted, and untracked files in one NUL-delimited `git status` call.
pub fn working_tree_status(repo_root: &Path) -> Result<WorkingTreeStatus> {
    let out = run_git_unquoted(repo_root, &["status", "--porcelain=v1", "-z", "-uall"])?;
    let mut status = WorkingTreeStatus::default();
    let mut entries = out.split('\0');

    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let bytes = entry.as_bytes();
        let (x, y) = (bytes[0], bytes[1]);
        let path = entry[3..].replace('\\', "/");

        if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
            let _orig_path = entries.next();
        }

        if x == b'?' && y == b'?' {
            status.untracked.push(path);
        } else if x == b'!' && y == b'!' {
            continue;
        } else if x == b'D' || y == b'D' {
            status.deleted.push(path);
        } else if matches!(x, b'A' | b'R' | b'C') {
            status.added.push(path);
        } else {
            status.modified.push(path);
        }
    }

    sort_dedup(&mut status.added);
    sort_dedup(&mut status.modified);
    sort_dedup(&mut status.deleted);
    sort_dedup(&mut status.untracked);
    Ok(status)
}

/// Lists unique relative paths changed under `scope`, normalized with forward slashes.
pub fn changed_files(repo_root: &Path, scope: DiffScope) -> Result<Vec<String>> {
    if scope == DiffScope::WorkingTree {
        let status = working_tree_status(repo_root)?;
        let mut unique = FxHashSet::default();
        unique.extend(status.added);
        unique.extend(status.modified);
        unique.extend(status.deleted);
        unique.extend(status.untracked);
        let mut files: Vec<String> = unique.into_iter().collect();
        files.sort();
        return Ok(files);
    }

    let mut unique = FxHashSet::default();
    match scope {
        DiffScope::Staged => {
            let out = run_git_unquoted(repo_root, &["diff", "--cached", "--name-only", "-z"])?;
            collect_nul_paths(&out, &mut unique);
        }
        DiffScope::Unstaged => {
            let out = run_git_unquoted(repo_root, &["diff", "--name-only", "-z"])?;
            collect_nul_paths(&out, &mut unique);
        }
        DiffScope::Head => {
            let diff = run_git_unquoted(repo_root, &["diff", "--name-only", "-z", "HEAD"])?;
            collect_nul_paths(&diff, &mut unique);
            let untracked = run_git_unquoted(
                repo_root,
                &["ls-files", "--others", "--exclude-standard", "-z"],
            )?;
            collect_nul_paths(&untracked, &mut unique);
        }
        DiffScope::WorkingTree => {}
    }
    let mut files: Vec<String> = unique.into_iter().collect();
    files.sort();
    Ok(files)
}

/// Returns the unified diff for a specific file (`staged` selects `--cached`).
pub fn file_diff(repo_root: &Path, file_path: &str, staged: bool) -> Result<String> {
    if staged {
        run_git_unquoted(repo_root, &["diff", "--cached", "--", file_path])
    } else {
        run_git_unquoted(repo_root, &["diff", "--", file_path])
    }
}

/// Returns the full unified diff across the repository.
pub fn all_diff(repo_root: &Path, staged: bool) -> Result<String> {
    if staged {
        run_git_unquoted(repo_root, &["diff", "--cached"])
    } else {
        run_git_unquoted(repo_root, &["diff"])
    }
}

fn sort_dedup(items: &mut Vec<String>) {
    items.sort();
    items.dedup();
}

fn collect_nul_paths(stdout: &str, out: &mut FxHashSet<String>) {
    for entry in stdout.split('\0') {
        if !entry.is_empty() {
            out.insert(entry.replace('\\', "/"));
        }
    }
}

pub(super) fn run_git_unquoted(cwd: &Path, args: &[&str]) -> Result<String> {
    let mut full_args = Vec::with_capacity(args.len() + 2);
    full_args.push("-c");
    full_args.push("core.quotePath=false");
    full_args.extend_from_slice(args);
    run_git(cwd, &full_args)
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
