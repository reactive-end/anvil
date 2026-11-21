//! Deterministic aggregation of `.harness/tasks/` into root `tasks.json`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::fs_util::{atomic_write_json, read_json_file};
use crate::ledger::tasks_root;
use crate::model::{ConsolidatedTask, ConsolidatedTasksFile, TaskEntry, TaskMeta, TaskStatus};

const TASKS_SCHEMA_VERSION: u32 = 1;
const MAX_SUMMARY_CHARS: usize = 480;

/// Rebuilds `<repo>/tasks.json` deterministically from `.harness/tasks/*/meta.json` and `entries/*.json`.
pub fn build_root_tasks_json(repo: &Path) -> Result<ConsolidatedTasksFile> {
    let records = read_all_task_records(repo)?;
    let consolidated = consolidate_tasks(records);
    atomic_write_json(&repo.join("tasks.json"), &consolidated)?;
    Ok(consolidated)
}

/// Reads every task directory's `meta.json` and `entries/*.json` from disk.
pub(crate) fn read_all_task_records(repo: &Path) -> Result<Vec<(TaskMeta, Vec<TaskEntry>)>> {
    let root = tasks_root(repo);
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut task_dirs: Vec<PathBuf> = fs::read_dir(&root)
        .with_context(|| format!("failed to read {}", root.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("meta.json").exists())
        .collect();
    task_dirs.sort();

    let mut records = Vec::with_capacity(task_dirs.len());
    for dir in task_dirs {
        let meta: TaskMeta = read_json_file(&dir.join("meta.json"))?;
        let entries = read_entries_dir(&dir.join("entries"))?;
        records.push((meta, entries));
    }
    Ok(records)
}

/// Deterministically consolidates task records sorted by `task_id` and `(timestamp, entry_id)`.
pub(crate) fn consolidate_tasks(
    mut records: Vec<(TaskMeta, Vec<TaskEntry>)>,
) -> ConsolidatedTasksFile {
    records.sort_by(|a, b| a.0.task_id.cmp(&b.0.task_id));
    let mut tasks = Vec::with_capacity(records.len());

    for (meta, entries) in records {
        tasks.push(consolidate_single_task(meta, entries));
    }

    let updated_at = tasks.iter().map(|t| t.updated_at).max();
    ConsolidatedTasksFile {
        version: TASKS_SCHEMA_VERSION,
        task_count: tasks.len(),
        updated_at,
        tasks,
    }
}

fn consolidate_single_task(meta: TaskMeta, raw_entries: Vec<TaskEntry>) -> ConsolidatedTask {
    let entries = dedup_and_sort_entries(raw_entries);
    let status = entries.last().map_or(TaskStatus::InProgress, |e| e.status);
    let updated_at = entries.last().map_or(meta.created_at, |e| e.timestamp);

    let contributors = collect_contributors(&meta.created_by, &entries);
    let branches = collect_branches(&entries);
    let files_touched = collect_files(&entries);
    let summary = build_concise_summary(&entries, &meta.request);

    ConsolidatedTask {
        task_id: meta.task_id,
        request: meta.request,
        status,
        created_by: meta.created_by,
        contributors,
        branches,
        files_touched,
        summary,
        entry_count: entries.len(),
        created_at: meta.created_at,
        updated_at,
        entries,
    }
}

fn read_entries_dir(entries_dir: &Path) -> Result<Vec<TaskEntry>> {
    if !entries_dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = fs::read_dir(entries_dir)
        .with_context(|| format!("failed to read {}", entries_dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let is_json = p.extension().and_then(|e| e.to_str()) == Some("json");
            let is_tmp = p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'));
            is_json && !is_tmp
        })
        .collect();
    paths.sort();

    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        out.push(read_json_file::<TaskEntry>(&path)?);
    }
    Ok(out)
}

fn dedup_and_sort_entries(raw: Vec<TaskEntry>) -> Vec<TaskEntry> {
    let mut seen = FxHashSet::with_capacity_and_hasher(raw.len(), Default::default());
    let mut unique = Vec::with_capacity(raw.len());
    for entry in raw {
        if seen.insert(entry.entry_id.clone()) {
            unique.push(entry);
        }
    }
    unique.sort_by(|a, b| {
        a.timestamp
            .cmp(&b.timestamp)
            .then_with(|| a.entry_id.cmp(&b.entry_id))
    });
    unique
}

fn collect_contributors(created_by: &str, entries: &[TaskEntry]) -> Vec<String> {
    let mut seen = FxHashSet::with_capacity_and_hasher(entries.len() + 1, Default::default());
    let mut contributors = Vec::new();
    if !created_by.is_empty() && seen.insert(created_by.to_owned()) {
        contributors.push(created_by.to_owned());
    }
    for entry in entries {
        if !entry.author.is_empty() && seen.insert(entry.author.clone()) {
            contributors.push(entry.author.clone());
        }
    }
    contributors
}

fn collect_branches(entries: &[TaskEntry]) -> Vec<String> {
    let mut seen = FxHashSet::with_capacity_and_hasher(entries.len(), Default::default());
    let mut branches = Vec::new();
    for entry in entries {
        if !entry.branch.is_empty() && seen.insert(entry.branch.clone()) {
            branches.push(entry.branch.clone());
        }
    }
    branches
}

fn collect_files(entries: &[TaskEntry]) -> Vec<String> {
    let mut seen = FxHashSet::default();
    let mut files = Vec::new();
    for entry in entries {
        for f in &entry.files_touched {
            if seen.insert(f.clone()) {
                files.push(f.clone());
            }
        }
    }
    files.sort();
    files
}

fn build_concise_summary(entries: &[TaskEntry], fallback_request: &str) -> String {
    if entries.is_empty() {
        return clamp_chars(fallback_request, MAX_SUMMARY_CHARS);
    }

    let mut latest_by_author: FxHashMap<&str, &str> = FxHashMap::default();
    let mut author_order: Vec<&str> = Vec::new();
    for entry in entries {
        if !latest_by_author.contains_key(entry.author.as_str()) {
            author_order.push(entry.author.as_str());
        }
        latest_by_author.insert(entry.author.as_str(), entry.summary.as_str());
    }

    if author_order.len() == 1 {
        let only = author_order[0];
        let text = latest_by_author.get(only).copied().unwrap_or(fallback_request);
        return clamp_chars(text, MAX_SUMMARY_CHARS);
    }

    let parts: Vec<String> = author_order
        .into_iter()
        .filter_map(|author| {
            latest_by_author
                .get(author)
                .map(|msg| format!("{author}: {msg}"))
        })
        .collect();
    clamp_chars(&parts.join(" | "), MAX_SUMMARY_CHARS)
}

fn clamp_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}
