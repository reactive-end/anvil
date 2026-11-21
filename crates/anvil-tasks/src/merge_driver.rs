//! Git merge driver installation and conflict-free `tasks.json` resolution.

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rustc_hash::FxHashMap;

use crate::builder::{consolidate_tasks, read_all_task_records};
use crate::fs_util::{atomic_write_bytes, atomic_write_json, read_json_file};
use crate::model::{ConsolidatedTasksFile, TaskEntry, TaskMeta};

const GITATTR_ENTRY: &str = "tasks.json merge=anvil-tasks";
const DEFAULT_DRIVER_CMD: &str = "anvil tasks merge-driver %O %A %B";

/// Installs `.gitattributes` and Git's `merge.anvil-tasks` driver in `repo`.
pub fn install_git_merge_driver(repo: &Path) -> Result<()> {
    install_git_merge_driver_with_cmd(repo, DEFAULT_DRIVER_CMD)
}

/// Installs `.gitattributes` and Git's `merge.anvil-tasks.driver` with a custom command.
pub fn install_git_merge_driver_with_cmd(repo: &Path, driver_cmd: &str) -> Result<()> {
    ensure_gitattributes(repo)?;
    run_git_config(
        repo,
        "merge.anvil-tasks.name",
        "Anvil deterministic tasks.json merge driver",
    )?;
    run_git_config(repo, "merge.anvil-tasks.driver", driver_cmd)?;
    Ok(())
}

/// Resolves a 3-way merge of `tasks.json` deterministically without conflict markers.
///
/// Combines tasks and entries from `.harness/tasks/` on disk with any tasks/entries
/// present in `current` (`%A`), `other` (`%B`), and `base` (`%O`), then writes the
/// consolidated result atomically to `current` (`%A`) as required by Git merge drivers.
pub fn resolve_merge(
    base: &Path,
    current: &Path,
    other: &Path,
    repo: &Path,
) -> Result<ConsolidatedTasksFile> {
    let mut by_task: FxHashMap<String, (TaskMeta, Vec<TaskEntry>)> = FxHashMap::default();

    for (meta, entries) in read_all_task_records(repo)? {
        merge_task_into(&mut by_task, meta, entries);
    }

    for candidate in [base, current, other] {
        if let Some(file) = read_optional_tasks_file(candidate) {
            for task in file.tasks {
                let meta = TaskMeta {
                    task_id: task.task_id.clone(),
                    created_by: task.created_by.clone(),
                    request: task.request.clone(),
                    created_at: task.created_at,
                };
                merge_task_into(&mut by_task, meta, task.entries);
            }
        }
    }

    let records: Vec<(TaskMeta, Vec<TaskEntry>)> = by_task.into_values().collect();
    let consolidated = consolidate_tasks(records);
    atomic_write_json(current, &consolidated)?;
    Ok(consolidated)
}

fn ensure_gitattributes(repo: &Path) -> Result<()> {
    let path = repo.join(".gitattributes");
    let existing = if path.exists() {
        fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?
    } else {
        String::new()
    };

    if existing.lines().any(|line| line.trim() == GITATTR_ENTRY) {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(GITATTR_ENTRY);
    updated.push('\n');
    atomic_write_bytes(&path, updated.as_bytes())
}

fn run_git_config(repo: &Path, key: &str, value: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["config", key, value])
        .current_dir(repo)
        .output()
        .with_context(|| format!("failed to execute git config {key}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git config {key} failed: {}", stderr.trim());
    }
    Ok(())
}

fn read_optional_tasks_file(path: &Path) -> Option<ConsolidatedTasksFile> {
    if !path.exists() {
        return None;
    }
    read_json_file::<ConsolidatedTasksFile>(path).ok()
}

fn merge_task_into(
    map: &mut FxHashMap<String, (TaskMeta, Vec<TaskEntry>)>,
    meta: TaskMeta,
    entries: Vec<TaskEntry>,
) {
    let key = meta.task_id.clone();
    match map.get_mut(&key) {
        Some((existing_meta, existing_entries)) => {
            if meta.created_at < existing_meta.created_at
                || (existing_meta.created_at == chrono::DateTime::<Utc>::UNIX_EPOCH
                    && meta.created_at > existing_meta.created_at)
            {
                *existing_meta = meta;
            }
            existing_entries.extend(entries);
        }
        None => {
            map.insert(key, (meta, entries));
        }
    }
}
