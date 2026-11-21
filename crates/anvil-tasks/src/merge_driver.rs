//! Git merge driver installation and conflict-free `tasks.json` resolution.

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use anvil_core::fs_util::atomic_write;
use chrono::Utc;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::builder::{consolidate_tasks, read_all_task_records};
use crate::model::{ConsolidatedTasksFile, TaskEntry, TaskMeta};
use crate::{atomic_write_json, read_json_file};

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
/// Combines live `.harness/tasks/` records with `current` (`%A`) and `other` (`%B`) while
/// never unioning `base` (`%O`), and filters out any task deleted in `current`, `other`, or
/// from `.harness/tasks/` on disk so deleted tasks are never resurrected.
pub fn resolve_merge(
    base: &Path,
    current: &Path,
    other: &Path,
    repo: &Path,
) -> Result<ConsolidatedTasksFile> {
    let disk_records = read_all_task_records(repo)?;
    let base_file = read_optional_tasks_file(base);
    let current_file = read_optional_tasks_file(current);
    let other_file = read_optional_tasks_file(other);
    let deleted_ids = collect_deleted_task_ids(
        &disk_records,
        base_file.as_ref(),
        current_file.as_ref(),
        other_file.as_ref(),
    );

    let mut by_task: FxHashMap<String, (TaskMeta, Vec<TaskEntry>)> = FxHashMap::default();
    for (meta, entries) in disk_records {
        if !deleted_ids.contains(&meta.task_id) {
            merge_task_into(&mut by_task, meta, entries);
        }
    }
    for file in [current_file, other_file].into_iter().flatten() {
        merge_file_tasks(&mut by_task, file, &deleted_ids);
    }

    let records: Vec<(TaskMeta, Vec<TaskEntry>)> = by_task.into_values().collect();
    let consolidated = consolidate_tasks(records);
    atomic_write_json(current, &consolidated)?;
    Ok(consolidated)
}

fn collect_deleted_task_ids(
    disk_records: &[(TaskMeta, Vec<TaskEntry>)],
    base: Option<&ConsolidatedTasksFile>,
    current: Option<&ConsolidatedTasksFile>,
    other: Option<&ConsolidatedTasksFile>,
) -> FxHashSet<String> {
    let mut deleted = FxHashSet::default();
    let current_ids = task_id_set(current);
    let other_ids = task_id_set(other);

    if let Some(base_file) = base {
        for task in &base_file.tasks {
            let missing_in_current = current.is_some() && !current_ids.contains(&task.task_id);
            let missing_in_other = other.is_some() && !other_ids.contains(&task.task_id);
            if missing_in_current || missing_in_other {
                deleted.insert(task.task_id.clone());
            }
        }
    }

    if !disk_records.is_empty() {
        let disk_ids: FxHashSet<&str> = disk_records
            .iter()
            .map(|(m, _)| m.task_id.as_str())
            .collect();
        for id in current_ids {
            if !disk_ids.contains(id.as_str()) {
                deleted.insert(id);
            }
        }
    }

    deleted
}

fn task_id_set(file: Option<&ConsolidatedTasksFile>) -> FxHashSet<String> {
    file.map(|f| f.tasks.iter().map(|t| t.task_id.clone()).collect())
        .unwrap_or_default()
}

fn merge_file_tasks(
    by_task: &mut FxHashMap<String, (TaskMeta, Vec<TaskEntry>)>,
    file: ConsolidatedTasksFile,
    deleted_ids: &FxHashSet<String>,
) {
    for task in file.tasks {
        if deleted_ids.contains(&task.task_id) {
            continue;
        }
        let meta = TaskMeta {
            task_id: task.task_id.clone(),
            created_by: task.created_by.clone(),
            request: task.request.clone(),
            created_at: task.created_at,
        };
        merge_task_into(by_task, meta, task.entries);
    }
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
    atomic_write(&path, updated.as_bytes())
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
