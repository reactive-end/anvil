//! `anvil worktree`: isolated Git worktrees inside `.harness/.worktrees/` for parallel agents.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::cli::subcmds::WorktreeSubcmd;
use crate::ui;

const WORKTREES_REL: &str = ".harness/.worktrees";

/// Executes `anvil worktree <add|list|remove>`.
pub fn execute(root: &Path, sub: WorktreeSubcmd) -> Result<()> {
    match sub {
        WorktreeSubcmd::Add { name, branch } => add_worktree(root, &name, branch.as_deref()),
        WorktreeSubcmd::List => list_worktrees(root),
        WorktreeSubcmd::Remove { name, force } => remove_worktree(root, &name, force),
    }
}

fn add_worktree(root: &Path, raw_name: &str, branch: Option<&str>) -> Result<()> {
    let name = validate_name(raw_name)?;
    let wt_dir = worktree_dir(root, &name);
    if wt_dir.exists() {
        bail!("worktree already exists at {}", wt_dir.display());
    }
    fs::create_dir_all(root.join(WORKTREES_REL))?;
    let wt_ignore = root.join(WORKTREES_REL).join(".gitignore");
    if !wt_ignore.exists() {
        fs::write(&wt_ignore, "*\n")?;
    }

    let branch_name = branch
        .map(str::to_owned)
        .unwrap_or_else(|| format!("anvil-wt/{name}"));
    let mut cmd = Command::new("git");
    cmd.arg("worktree").arg("add").arg("-b").arg(&branch_name).arg(&wt_dir);
    let out = cmd
        .current_dir(root)
        .output()
        .context("failed to invoke git worktree add")?;
    if !out.status.success() {
        bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    copy_if_present(&root.join(".harness/config.json"), &wt_dir.join(".harness/config.json"))?;
    copy_if_present(&root.join(".env"), &wt_dir.join(".env"))?;

    println!(
        "{} {} (branch={})",
        ui::green("✓ worktree ready:"),
        ui::cyan(&wt_dir.display().to_string()),
        branch_name
    );
    Ok(())
}

fn list_worktrees(root: &Path) -> Result<()> {
    let out = Command::new("git")
        .args(["worktree", "list"])
        .current_dir(root)
        .output()
        .context("failed to run git worktree list")?;
    if !out.status.success() {
        bail!(
            "git worktree list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    print!("{}", String::from_utf8_lossy(&out.stdout));
    Ok(())
}

fn remove_worktree(root: &Path, raw_name: &str, force: bool) -> Result<()> {
    let name = validate_name(raw_name)?;
    let wt_dir = worktree_dir(root, &name);
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    let out = Command::new("git")
        .args(args)
        .arg(&wt_dir)
        .current_dir(root)
        .output()
        .context("failed to run git worktree remove")?;
    if !out.status.success() {
        bail!(
            "git worktree remove failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    cleanup_worktree_branch(root, &name, force);
    println!("removed worktree {}", ui::cyan(&name));
    Ok(())
}

fn cleanup_worktree_branch(root: &Path, name: &str, force: bool) {
    let branch_name = format!("anvil-wt/{name}");
    let branch_ref = format!("refs/heads/{branch_name}");
    let exists = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", &branch_ref])
        .current_dir(root)
        .output()
        .is_ok_and(|o| o.status.success());
    if !exists {
        return;
    }
    let flag = if force { "-D" } else { "-d" };
    if let Ok(del) = Command::new("git")
        .args(["branch", flag, &branch_name])
        .current_dir(root)
        .output()
        && !del.status.success()
    {
        eprintln!(
            "warning: worktree removed but branch `{branch_name}` was kept: {}",
            String::from_utf8_lossy(&del.stderr).trim()
        );
    }
}

fn validate_name(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("worktree name must contain only [a-zA-Z0-9_-]");
    }
    Ok(trimmed.to_owned())
}

fn worktree_dir(root: &Path, name: &str) -> PathBuf {
    root.join(WORKTREES_REL).join(name)
}

fn copy_if_present(src: &Path, dst: &Path) -> Result<()> {
    if !src.is_file() {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(src, dst)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn remove_worktree_deletes_default_branch_allowing_re_add() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        for args in [
            &["init", "-b", "main"][..],
            &["config", "user.name", "Test"][..],
            &["config", "user.email", "t@e.co"][..],
        ] {
            let out = Command::new("git").args(args).current_dir(root).output().unwrap();
            assert!(out.status.success());
        }
        fs::write(root.join("README.md"), "hello\n").unwrap();
        let add_out = Command::new("git").args(["add", "-A"]).current_dir(root).output().unwrap();
        assert!(add_out.status.success());
        let ci_out = Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(ci_out.status.success());

        add_worktree(root, "agent-readd", None).expect("first add");
        remove_worktree(root, "agent-readd", false).expect("remove deletes default branch");
        add_worktree(root, "agent-readd", None).expect("second add with same name succeeds");
        remove_worktree(root, "agent-readd", true).expect("cleanup");
    }
}
