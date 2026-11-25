//! Git merge driver installation and conflict-free `tasks.json` resolution.

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use anvil_core::fs_util::atomic_write;
use chrono::{DateTime, Utc};
use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::Value;

use crate::atomic_write_json;
use crate::builder::{consolidate_tasks, read_all_task_records};
use crate::ledger::compare_task_ids;
use crate::model::{
    ConsolidatedTasksFile, TASKS_INDEX_SCHEMA_VERSION, TaskIndexEntry, TaskStatus, TasksIndexFile,
};
use crate::progress::task_progress_rel_path;

const GITATTR_ENTRY: &str = "tasks.json merge=anvil-tasks";
/// Default Git merge driver command with POSIX `sh` fallback to `git merge-file`
/// when `anvil` is unavailable on `PATH`.
pub const DEFAULT_DRIVER_CMD: &str = "if command -v anvil >/dev/null 2>&1; then anvil tasks merge-driver %O %A %B; else git merge-file %A %O %B; fi";

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

/// Resolves a 3-way merge of `tasks.json` deterministically into a compact v2 index.
///
/// Combines live `.harness/tasks/` records with `current` (`%A`) and `other` (`%B`) while
/// never unioning `base` (`%O`), accepts both v2 index and legacy v1 `tasks.json` inputs,
/// and filters out deleted tasks so they are never resurrected.
pub fn resolve_merge(
    base: &Path,
    current: &Path,
    other: &Path,
    repo: &Path,
) -> Result<TasksIndexFile> {
    let disk_records = read_all_task_records(repo)?;
    let disk_index = TasksIndexFile::from(&consolidate_tasks(disk_records));
    let base_file = read_optional_index(base);
    let current_file = read_optional_index(current);
    let other_file = read_optional_index(other);

    let deleted_ids = collect_deleted_task_ids(
        &disk_index,
        base_file.as_ref(),
        current_file.as_ref(),
        other_file.as_ref(),
    );

    let disk_map = index_by_id(Some(disk_index));
    let base_map = index_by_id(base_file);
    let cur_map = index_by_id(current_file);
    let oth_map = index_by_id(other_file);

    let mut all_ids: Vec<String> = collect_unique_ids(&[&disk_map, &cur_map, &oth_map])
        .into_iter()
        .filter(|id| !deleted_ids.contains(id))
        .collect();
    all_ids.sort_by(|a, b| compare_task_ids(a, b));

    let tasks: Vec<TaskIndexEntry> = all_ids
        .into_iter()
        .filter_map(|id| {
            merge_task_entry(
                &id,
                base_map.get(&id),
                disk_map.get(&id),
                cur_map.get(&id),
                oth_map.get(&id),
            )
        })
        .collect();

    let updated_at = tasks.iter().map(|t| t.updated_at).max();
    let merged = TasksIndexFile {
        version: TASKS_INDEX_SCHEMA_VERSION,
        task_count: tasks.len(),
        updated_at,
        tasks,
    };
    atomic_write_json(current, &merged)?;
    Ok(merged)
}

fn index_by_id(file: Option<TasksIndexFile>) -> FxHashMap<String, TaskIndexEntry> {
    let mut map = FxHashMap::default();
    if let Some(f) = file {
        for task in f.tasks {
            map.insert(task.task_id.clone(), task);
        }
    }
    map
}

fn collect_unique_ids(maps: &[&FxHashMap<String, TaskIndexEntry>]) -> Vec<String> {
    let mut seen = FxHashSet::default();
    let mut ids = Vec::new();
    for map in maps {
        for key in map.keys() {
            if seen.insert(key.clone()) {
                ids.push(key.clone());
            }
        }
    }
    ids
}

fn collect_deleted_task_ids(
    disk_index: &TasksIndexFile,
    base: Option<&TasksIndexFile>,
    current: Option<&TasksIndexFile>,
    other: Option<&TasksIndexFile>,
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

    if !disk_index.tasks.is_empty() {
        let disk_ids: FxHashSet<&str> =
            disk_index.tasks.iter().map(|t| t.task_id.as_str()).collect();
        for id in current_ids {
            if !disk_ids.contains(id.as_str()) {
                deleted.insert(id);
            }
        }
    }
    deleted
}

fn task_id_set(file: Option<&TasksIndexFile>) -> FxHashSet<String> {
    file.map(|f| f.tasks.iter().map(|t| t.task_id.clone()).collect())
        .unwrap_or_default()
}

fn merge_task_entry(
    task_id: &str,
    base: Option<&TaskIndexEntry>,
    disk: Option<&TaskIndexEntry>,
    cur: Option<&TaskIndexEntry>,
    oth: Option<&TaskIndexEntry>,
) -> Option<TaskIndexEntry> {
    let candidates: Vec<&TaskIndexEntry> = [disk, cur, oth].into_iter().flatten().collect();
    let seed = *candidates.first()?;
    let (created_by, created_at, request, project) = pick_origin_meta(&candidates);
    let (status, summary, evidence, updated_at) = pick_latest_state(&candidates, &seed.summary);
    let contributors = union_unique(
        candidates
            .iter()
            .map(|t| t.contributors.as_slice())
            .collect(),
    );
    let branches = union_unique(candidates.iter().map(|t| t.branches.as_slice()).collect());
    let entry_count = merge_counts(
        base.map_or(0, |t| t.entry_count),
        disk.map_or(0, |t| t.entry_count),
        cur.map_or(0, |t| t.entry_count),
        oth.map_or(0, |t| t.entry_count),
    );
    let files_count = merge_counts(
        base.map_or(0, |t| t.files_count),
        disk.map_or(0, |t| t.files_count),
        cur.map_or(0, |t| t.files_count),
        oth.map_or(0, |t| t.files_count),
    );

    Some(TaskIndexEntry {
        task_id: task_id.to_owned(),
        request,
        project,
        status,
        created_by,
        contributors,
        branches,
        summary,
        evidence,
        entry_count,
        files_count,
        progress_file: task_progress_rel_path(task_id),
        created_at,
        updated_at,
    })
}

fn pick_origin_meta(
    candidates: &[&TaskIndexEntry],
) -> (String, DateTime<Utc>, String, Option<String>) {
    let mut best = candidates[0];
    for &cand in &candidates[1..] {
        let cand_better = cand.created_at > DateTime::<Utc>::UNIX_EPOCH
            && (best.created_at == DateTime::<Utc>::UNIX_EPOCH || cand.created_at < best.created_at);
        if cand_better {
            best = cand;
        }
    }
    let project = candidates.iter().find_map(|c| c.project.clone());
    (
        best.created_by.clone(),
        best.created_at,
        best.request.clone(),
        project,
    )
}

fn pick_latest_state(
    candidates: &[&TaskIndexEntry],
    fallback_summary: &str,
) -> (TaskStatus, String, Option<String>, DateTime<Utc>) {
    let mut best = candidates[0];
    for &cand in &candidates[1..] {
        let is_newer = cand.updated_at > best.updated_at
            || (cand.updated_at == best.updated_at && cand.status > best.status);
        if is_newer {
            best = cand;
        }
    }
    let evidence = best.evidence.clone().or_else(|| {
        candidates
            .iter()
            .filter_map(|c| c.evidence.as_ref())
            .next()
            .cloned()
    });
    let summary = if best.summary.trim().is_empty() {
        fallback_summary.to_owned()
    } else {
        best.summary.clone()
    };
    (best.status, summary, evidence, best.updated_at)
}

fn union_unique(slices: Vec<&[String]>) -> Vec<String> {
    let mut seen = FxHashSet::default();
    let mut out = Vec::new();
    for slice in slices {
        for item in slice {
            if !item.is_empty() && seen.insert(item.clone()) {
                out.push(item.clone());
            }
        }
    }
    out
}

fn merge_counts(base: usize, disk: usize, cur: usize, oth: usize) -> usize {
    if base > 0 || (cur > 0 && oth > 0) {
        let delta = base + cur.saturating_sub(base) + oth.saturating_sub(base);
        disk.max(delta)
    } else {
        disk.max(cur).max(oth)
    }
}

fn read_optional_index(path: &Path) -> Option<TasksIndexFile> {
    let raw = fs::read_to_string(path).ok()?;
    let val: Value = serde_json::from_str(&raw).ok()?;
    let version = val.get("version").and_then(Value::as_u64).unwrap_or(1);
    let has_v1_entries = val
        .get("tasks")
        .and_then(Value::as_array)
        .is_some_and(|arr| arr.iter().any(|t| t.get("entries").is_some()));
    if version < u64::from(TASKS_INDEX_SCHEMA_VERSION) || has_v1_entries {
        return serde_json::from_value::<ConsolidatedTasksFile>(val)
            .ok()
            .map(|v1| TasksIndexFile::from(&v1));
    }
    serde_json::from_value::<TasksIndexFile>(val).ok()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_driver_cmd_includes_git_merge_file_fallback() {
        assert!(DEFAULT_DRIVER_CMD.contains("command -v anvil"));
        assert!(DEFAULT_DRIVER_CMD.contains("anvil tasks merge-driver %O %A %B"));
        assert!(DEFAULT_DRIVER_CMD.contains("git merge-file %A %O %B"));
    }
}
