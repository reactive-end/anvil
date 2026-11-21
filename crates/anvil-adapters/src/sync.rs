//! Non-destructive repository initialization and thin-adapter synchronization.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::atomic_write;
use anvil_core::stack_detector;

use crate::{antigravity, claude_code, templates};

const MANAGED_START: &str = "<!-- anvil:managed:start -->";
const MANAGED_END: &str = "<!-- anvil:managed:end -->";

/// Summary of actions taken by [`init_repo`] or [`sync_repo`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    pub created_dirs: Vec<PathBuf>,
    pub written_files: Vec<PathBuf>,
    pub preserved_skills: Vec<String>,
}

/// Initializes `.harness/` in `repo_root` and synchronizes all thin adapters.
pub fn init_repo(repo_root: &Path) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    ensure_harness_skeleton(repo_root, &mut report)?;

    let config_path = AnvilConfig::path(repo_root);
    if !config_path.exists() {
        let detection = stack_detector::detect_workspace(repo_root)?;
        let mut config = AnvilConfig::default();
        detection.apply_to_config(&mut config);
        config.save(repo_root)?;
        report.written_files.push(config_path);
    }

    sync_into(repo_root, &mut report)?;
    Ok(report)
}

/// Synchronizes `.harness/skills/`, adapter configs, `.gitignore`, `AGENTS.md`, and `CLAUDE.md`
/// without deleting pre-existing user skills, hooks, or MCP servers.
pub fn sync_repo(repo_root: &Path) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    ensure_harness_skeleton(repo_root, &mut report)?;
    sync_into(repo_root, &mut report)?;
    Ok(report)
}

fn sync_into(repo_root: &Path, report: &mut SyncReport) -> Result<()> {
    write_embedded_skills(repo_root, report)?;
    sync_skills_targets(repo_root, report)?;

    for (path, changed) in [
        (antigravity::config::hooks_config_path(repo_root), antigravity::config::merge_hooks_config(repo_root)?),
        (antigravity::config::mcp_config_path(repo_root), antigravity::config::merge_mcp_config(repo_root)?),
        (claude_code::config::settings_config_path(repo_root), claude_code::config::merge_settings_config(repo_root)?),
        (claude_code::config::mcp_config_path(repo_root), claude_code::config::merge_mcp_config(repo_root)?),
    ] {
        if changed {
            report.written_files.push(path);
        }
    }

    sync_gitignore(repo_root, report)?;
    sync_managed_doc(&repo_root.join("AGENTS.md"), templates::AGENTS_MD, report)?;
    sync_managed_doc(&repo_root.join("CLAUDE.md"), templates::CLAUDE_MD, report)?;
    Ok(())
}

pub(crate) fn write_if_changed(path: &Path, content: &str) -> Result<bool> {
    if fs::read_to_string(path).is_ok_and(|existing| existing == content) {
        return Ok(false);
    }
    atomic_write(path, content.as_bytes())?;
    Ok(true)
}

fn ensure_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("failed to create directory {}", dir.display()))
}

fn ensure_harness_skeleton(repo_root: &Path, report: &mut SyncReport) -> Result<()> {
    let harness = repo_root.join(".harness");
    for rel in ["rules", "candidates", "skills", "memory/chunks", "tasks", ".state"] {
        let dir = harness.join(rel);
        if !dir.exists() {
            ensure_dir(&dir)?;
            report.created_dirs.push(dir);
        }
    }
    Ok(())
}

fn write_embedded_skills(repo_root: &Path, report: &mut SyncReport) -> Result<()> {
    let skills_root = repo_root.join(".harness").join("skills");
    for skill in templates::EMBEDDED_SKILLS {
        let target = skills_root.join(skill.name).join("SKILL.md");
        if write_if_changed(&target, skill.skill_md)? {
            report.written_files.push(target);
        }
    }
    Ok(())
}

fn sync_skills_targets(repo_root: &Path, report: &mut SyncReport) -> Result<()> {
    let source_root = repo_root.join(".harness").join("skills");
    let source_names = list_subdir_names(&source_root)?;

    let targets = [
        repo_root.join(".agents").join("skills"),
        repo_root.join(".claude").join("skills"),
    ];

    let mut preserved = BTreeSet::new();
    for target_root in &targets {
        ensure_dir(target_root)?;
        for existing_name in list_subdir_names(target_root)? {
            if !source_names.contains(&existing_name) {
                preserved.insert(existing_name);
            }
        }
        for skill_name in &source_names {
            let src_dir = source_root.join(skill_name);
            let dst_dir = target_root.join(skill_name);
            copy_dir_non_destructively(&src_dir, &dst_dir, report)?;
        }
    }
    report.preserved_skills.extend(preserved);
    Ok(())
}

fn list_subdir_names(dir: &Path) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    if !dir.exists() {
        return Ok(names);
    }
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            names.insert(name.to_owned());
        }
    }
    Ok(names)
}

fn copy_dir_non_destructively(
    src: &Path,
    dst: &Path,
    report: &mut SyncReport,
) -> Result<()> {
    ensure_dir(dst)?;
    for entry in fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_non_destructively(&src_path, &dst_path, report)?;
        } else {
            let bytes = fs::read(&src_path)
                .with_context(|| format!("failed to read {}", src_path.display()))?;
            let unchanged = fs::read(&dst_path).is_ok_and(|existing| existing == bytes);
            if !unchanged {
                atomic_write(&dst_path, &bytes)?;
                report.written_files.push(dst_path);
            }
        }
    }
    Ok(())
}

fn sync_gitignore(repo_root: &Path, report: &mut SyncReport) -> Result<()> {
    let path = repo_root.join(".gitignore");
    let existing = if path.exists() {
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?
    } else {
        String::new()
    };

    let has_state = existing
        .lines()
        .map(str::trim)
        .any(|line| line == ".harness/.state/" || line == ".harness/.state");
    if has_state {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(".harness/.state/\n");
    if write_if_changed(&path, &updated)? {
        report.written_files.push(path);
    }
    Ok(())
}

fn sync_managed_doc(path: &Path, template: &str, report: &mut SyncReport) -> Result<()> {
    let existing = if path.exists() {
        Some(
            fs::read_to_string(path)
                .with_context(|| format!("failed to read {}", path.display()))?,
        )
    } else {
        None
    };

    let merged = merge_managed_markdown(existing.as_deref(), template);
    if write_if_changed(path, &merged)? {
        report.written_files.push(path.to_path_buf());
    }
    Ok(())
}

fn merge_managed_markdown(existing: Option<&str>, template: &str) -> String {
    let Some(current) = existing else {
        return template.to_owned();
    };
    if current.trim().is_empty() {
        return template.to_owned();
    }
    if let (Some(start), Some(end)) = (current.find(MANAGED_START), current.find(MANAGED_END))
        && start < end
    {
        let before = &current[..start];
        let after = &current[end + MANAGED_END.len()..];
        let replacement = template.trim_end();
        return format!("{before}{replacement}{after}");
    }
    let sep = if current.ends_with('\n') { "\n" } else { "\n\n" };
    format!("{current}{sep}{template}")
}
