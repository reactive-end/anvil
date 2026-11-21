//! Conflict-free task ledger operations on `.harness/tasks/<id>/`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rustc_hash::FxHashSet;
use ulid::Ulid;

use crate::fs_util::{atomic_write_bytes, atomic_write_json, read_json_file};
use crate::identity::{resolve_author, sanitize_author};
use crate::model::{TaskEntry, TaskMeta, TaskStatus};

/// Returns `.harness/tasks`.
pub fn tasks_root(repo: &Path) -> PathBuf {
    repo.join(".harness").join("tasks")
}

/// Returns `.harness/tasks/<task_id>`.
pub fn task_dir(repo: &Path, task_id: &str) -> PathBuf {
    tasks_root(repo).join(task_id)
}

/// Creates a new task directory, writes `meta.json`, and records the initial entry.
pub fn start_task(repo: &Path, request: &str) -> Result<TaskMeta> {
    let author = resolve_author(repo)?;
    start_task_as(repo, &author, request)
}

/// Creates a new task attributed to an explicit `author`.
pub fn start_task_as(repo: &Path, author: &str, request: &str) -> Result<TaskMeta> {
    let clean_author = sanitize_author(author);
    let now = Utc::now();
    let ulid_str = Ulid::new().to_string();
    // 16 Crockford chars = 10 timestamp chars + 6 random chars (sortable and collision-free).
    let short_ulid = &ulid_str[..16];
    let task_id = format!("T-{}-{short_ulid}", now.format("%Y%m%d"));

    let dir = task_dir(repo, &task_id);
    let entries_dir = dir.join("entries");
    fs::create_dir_all(&entries_dir)
        .with_context(|| format!("failed to create {}", entries_dir.display()))?;

    let meta = TaskMeta {
        task_id: task_id.clone(),
        created_by: clean_author.clone(),
        request: request.trim().to_owned(),
        created_at: now,
    };
    atomic_write_json(&dir.join("meta.json"), &meta)?;

    let initial_summary = format!("Started task: {}", truncate_chars(request.trim(), 120));
    append_entry_as(
        repo,
        &task_id,
        &clean_author,
        TaskStatus::InProgress,
        &initial_summary,
        &[],
    )?;

    Ok(meta)
}

/// Appends an immutable progress entry for `task_id` attributed to the resolved author.
pub fn append_entry(
    repo: &Path,
    task_id: &str,
    status: TaskStatus,
    summary: &str,
    files_touched: &[String],
) -> Result<TaskEntry> {
    let author = resolve_author(repo)?;
    append_entry_as(repo, task_id, &author, status, summary, files_touched)
}

/// Appends an immutable entry `.harness/tasks/<id>/entries/<author>-<ulid>.json` for `author`.
pub fn append_entry_as(
    repo: &Path,
    task_id: &str,
    author: &str,
    status: TaskStatus,
    summary: &str,
    files_touched: &[String],
) -> Result<TaskEntry> {
    let dir = task_dir(repo, task_id);
    let meta_path = dir.join("meta.json");
    if !meta_path.exists() {
        bail!("task `{task_id}` not found at {}", meta_path.display());
    }

    let clean_author = sanitize_author(author);
    let entry_id = Ulid::new().to_string();
    let branch = anvil_core::git::current_branch(repo).unwrap_or_else(|_| "HEAD".to_owned());
    let entry = TaskEntry {
        entry_id: entry_id.clone(),
        task_id: task_id.to_owned(),
        author: clean_author.clone(),
        status,
        branch,
        summary: summary.trim().to_owned(),
        files_touched: normalize_files(files_touched),
        timestamp: Utc::now(),
    };

    let entry_path = dir
        .join("entries")
        .join(format!("{clean_author}-{entry_id}.json"));
    if entry_path.exists() {
        bail!("immutable entry already exists at {}", entry_path.display());
    }
    atomic_write_json(&entry_path, &entry)?;
    Ok(entry)
}

/// Writes an isolated per-author plan file `.harness/tasks/<id>/plan-<author>.md`.
pub fn write_task_plan(
    repo: &Path,
    task_id: &str,
    author: &str,
    markdown: &str,
) -> Result<PathBuf> {
    let dir = task_dir(repo, task_id);
    if !dir.join("meta.json").exists() {
        bail!("task `{task_id}` not found at {}", dir.display());
    }
    let clean_author = sanitize_author(author);
    let plan_path = dir.join(format!("plan-{clean_author}.md"));
    atomic_write_bytes(&plan_path, markdown.as_bytes())?;
    Ok(plan_path)
}

/// Lists all task metadata records under `.harness/tasks/`, sorted by `task_id`.
pub fn list_tasks(repo: &Path) -> Result<Vec<TaskMeta>> {
    let root = tasks_root(repo);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut dirs: Vec<PathBuf> = fs::read_dir(&root)
        .with_context(|| format!("failed to read {}", root.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("meta.json").exists())
        .collect();
    dirs.sort();

    let mut metas = Vec::with_capacity(dirs.len());
    for dir in dirs {
        metas.push(read_json_file::<TaskMeta>(&dir.join("meta.json"))?);
    }
    metas.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    Ok(metas)
}

fn normalize_files(files: &[String]) -> Vec<String> {
    let mut seen = FxHashSet::with_capacity_and_hasher(files.len(), Default::default());
    let mut out = Vec::with_capacity(files.len());
    for raw in files {
        let norm = raw.trim().replace('\\', "/");
        if !norm.is_empty() && seen.insert(norm.clone()) {
            out.push(norm);
        }
    }
    out.sort();
    out
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut iter = text.chars();
    let taken: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{taken}...")
    } else {
        taken
    }
}
