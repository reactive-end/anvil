//! `anvil checkpoint`: lightweight Git ref snapshots (`refs/anvil/checkpoints/*`) and rewind.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use ulid::Ulid;

use crate::cli::subcmds::CheckpointSubcmd;
use crate::ui;

const REF_PREFIX: &str = "refs/anvil/checkpoints/";

struct TempIndexGuard(PathBuf);

impl Drop for TempIndexGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Executes `anvil checkpoint <save|list|rewind|drop>`.
pub fn execute(root: &Path, sub: CheckpointSubcmd) -> Result<()> {
    match sub {
        CheckpointSubcmd::Save { label } => {
            let ref_name = create_checkpoint(root, label.as_deref())?;
            println!("{} saved {}", ui::green("✓ checkpoint:"), ui::cyan(&ref_name));
            Ok(())
        }
        CheckpointSubcmd::List => list_checkpoints(root),
        CheckpointSubcmd::Rewind { target } => rewind_checkpoint(root, target.as_deref()),
        CheckpointSubcmd::Drop { target } => drop_checkpoint(root, &target),
    }
}

/// Snapshots the entire working tree into an isolated Git ref without mutating `HEAD` or `.git/index`.
pub fn create_checkpoint(root: &Path, label: Option<&str>) -> Result<String> {
    let git_dir = resolve_git_dir(root)?;
    let state_dir = root.join(".harness").join(".state");
    fs::create_dir_all(&state_dir)?;
    let temp_index = state_dir.join(format!("ckpt-index-{}", Ulid::new()));
    let real_index = git_dir.join("index");
    if real_index.is_file() {
        fs::copy(&real_index, &temp_index)?;
    }
    let _guard = TempIndexGuard(temp_index.clone());

    run_git_env(root, &["add", "-A"], &temp_index)?;
    let tree = run_git_env(root, &["write-tree"], &temp_index)?
        .trim()
        .to_owned();

    let slug = sanitize_label(label.unwrap_or("snapshot"));
    let ts = Utc::now().format("%Y%m%d-%H%M%S");
    let msg = format!("anvil checkpoint: {slug}");

    let mut commit_args = vec!["commit-tree", &tree, "-m", &msg];
    let head_sha = run_git(root, &["rev-parse", "--verify", "HEAD"])
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    if let Some(ref parent) = head_sha {
        commit_args.push("-p");
        commit_args.push(parent);
    }
    let commit_sha = run_git(root, &commit_args)?.trim().to_owned();
    let ref_name = format!("{REF_PREFIX}{ts}-{slug}");
    run_git(root, &["update-ref", &ref_name, &commit_sha])?;
    Ok(ref_name)
}

fn list_checkpoints(root: &Path) -> Result<()> {
    let refs = read_checkpoint_refs(root)?;
    if refs.is_empty() {
        println!("no checkpoints found");
        return Ok(());
    }
    println!("{}", ui::accent("anvil checkpoints:"));
    for (ref_name, short_sha, subject) in refs {
        println!(
            "  - {} ({}) {}",
            ui::cyan(ref_name.trim_start_matches(REF_PREFIX)),
            ui::dim(&short_sha),
            subject
        );
    }
    Ok(())
}

fn rewind_checkpoint(root: &Path, target: Option<&str>) -> Result<()> {
    let refs = read_checkpoint_refs(root)?;
    let selected = match target {
        Some(t) => resolve_target_ref(&refs, t)?,
        None => refs
            .first()
            .map(|(r, _, _)| r.clone())
            .ok_or_else(|| anyhow::anyhow!("no checkpoints available to rewind"))?,
    };
    let backup = create_checkpoint(root, Some("pre-rewind"))?;
    run_git(root, &["checkout", &selected, "--", "."])?;
    remove_files_added_since(root, &selected, &backup)?;
    println!(
        "{} restored {} (safety backup saved at {})",
        ui::green("✓ rewind:"),
        ui::cyan(selected.trim_start_matches(REF_PREFIX)),
        ui::dim(backup.trim_start_matches(REF_PREFIX))
    );
    Ok(())
}

fn remove_files_added_since(root: &Path, base_ref: &str, newer_ref: &str) -> Result<()> {
    let added = run_git(
        root,
        &["diff", "--name-only", "--diff-filter=A", base_ref, newer_ref],
    )?;
    for rel in added.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if rel.starts_with(".harness/.state/") || rel.starts_with(".harness/.worktrees/") {
            continue;
        }
        let target = root.join(rel);
        if target.is_file() {
            fs::remove_file(&target)?;
            prune_empty_parents(root, target.parent());
        }
    }
    Ok(())
}

fn prune_empty_parents(root: &Path, mut cur: Option<&Path>) {
    while let Some(dir) = cur {
        if dir == root || !dir.starts_with(root) {
            break;
        }
        let is_empty = fs::read_dir(dir).is_ok_and(|mut it| it.next().is_none());
        if !is_empty || fs::remove_dir(dir).is_err() {
            break;
        }
        cur = dir.parent();
    }
}

fn drop_checkpoint(root: &Path, target: &str) -> Result<()> {
    let refs = read_checkpoint_refs(root)?;
    if target == "all" {
        for (ref_name, _, _) in refs {
            run_git(root, &["update-ref", "-d", &ref_name])?;
        }
        println!("{}", ui::green("dropped all checkpoints"));
        return Ok(());
    }
    let resolved = resolve_target_ref(&refs, target)?;
    run_git(root, &["update-ref", "-d", &resolved])?;
    println!("dropped {}", ui::cyan(&resolved));
    Ok(())
}

fn resolve_target_ref(refs: &[(String, String, String)], target: &str) -> Result<String> {
    let full = if target.starts_with(REF_PREFIX) {
        target.to_owned()
    } else {
        format!("{REF_PREFIX}{target}")
    };
    refs.iter()
        .find(|(r, sha, _)| *r == full || *r == target || sha.starts_with(target))
        .map(|(r, _, _)| r.clone())
        .ok_or_else(|| anyhow::anyhow!("checkpoint `{target}` not found"))
}

fn read_checkpoint_refs(root: &Path) -> Result<Vec<(String, String, String)>> {
    let raw = run_git(
        root,
        &[
            "for-each-ref",
            "--sort=-creatordate",
            "--format=%(refname)%09%(objectname:short)%09%(subject)",
            REF_PREFIX,
        ],
    )?;
    let mut out = Vec::new();
    for line in raw.lines() {
        let mut parts = line.splitn(3, '\t');
        if let (Some(r), Some(sha), Some(subj)) = (parts.next(), parts.next(), parts.next()) {
            out.push((r.to_owned(), sha.to_owned(), subj.to_owned()));
        }
    }
    Ok(out)
}

fn resolve_git_dir(root: &Path) -> Result<PathBuf> {
    let raw = run_git(root, &["rev-parse", "--git-dir"])?;
    let p = PathBuf::from(raw.trim());
    Ok(if p.is_absolute() { p } else { root.join(p) })
}

fn sanitize_label(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "snapshot".to_owned()
    } else {
        trimmed.chars().take(40).collect()
    }
}

fn run_git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run_git_env(root: &Path, args: &[&str], index_file: &Path) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_INDEX_FILE", index_file)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
