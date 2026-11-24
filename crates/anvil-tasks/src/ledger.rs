//! Conflict-free task ledger operations on `.harness/tasks/<id>/`.

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rustc_hash::FxHashSet;
use ulid::Ulid;

use anvil_core::fs_util::{atomic_write, sanitize_author, with_file_lock};

use crate::active::{load_active_task, save_active_task};
use crate::identity::resolve_author;
use crate::model::{TaskEntry, TaskMeta, TaskStatus};
use crate::{atomic_write_json, read_json_file};

const ID_LOCK_TIMEOUT_MS: u64 = 5000;

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

/// Compares two task IDs naturally (`T-002 < T-010`, `F001 < F038`, fallback lexicographic).
pub fn compare_task_ids(a: &str, b: &str) -> Ordering {
    let (pa, na) = split_id_prefix_num(a);
    let (pb, nb) = split_id_prefix_num(b);
    pa.cmp(pb)
        .then_with(|| match (na, nb) {
            (Some(x), Some(y)) => x.cmp(&y),
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        })
        .then_with(|| a.cmp(b))
}

fn split_id_prefix_num(id: &str) -> (&str, Option<u64>) {
    let digit_start = id
        .rfind(|c: char| !c.is_ascii_digit())
        .map_or(0, |i| i + 1);
    if digit_start < id.len()
        && let Ok(num) = id[digit_start..].parse::<u64>()
    {
        return (&id[..digit_start], Some(num));
    }
    (id, None)
}

/// Creates a new task attributed to `author` with the next short sequential ID (`T-001`, ...).
pub fn start_task_as(repo: &Path, author: &str, request: &str) -> Result<TaskMeta> {
    start_task_with_id(repo, author, request, None)
}

/// Creates a new task with an optional custom code (e.g. `F039`) or the next `T-NNN` sequence.
pub fn start_task_with_id(
    repo: &Path,
    author: &str,
    request: &str,
    explicit_id: Option<&str>,
) -> Result<TaskMeta> {
    let clean_author = sanitize_author(author);
    let now = Utc::now();
    let lock_path = repo.join(".harness/.state/tasks-id.lock");

    let meta = with_file_lock(&lock_path, ID_LOCK_TIMEOUT_MS, || {
        let task_id = match explicit_id {
            Some(raw) => {
                let clean = sanitize_explicit_id(raw)?;
                if task_dir(repo, &clean).join("meta.json").exists() {
                    bail!("task `{clean}` already exists");
                }
                clean
            }
            None => next_sequential_task_id(repo)?,
        };
        let dir = task_dir(repo, &task_id);
        fs::create_dir_all(dir.join("entries"))
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let meta = TaskMeta {
            task_id,
            created_by: clean_author.clone(),
            request: request.trim().to_owned(),
            project: None,
            behavior: None,
            verify: None,
            created_at: now,
        };
        atomic_write_json(&dir.join("meta.json"), &meta)?;
        Ok(meta)
    })?;

    let initial_summary = format!("Started task: {}", truncate_chars(request.trim(), 120));
    append_entry_as(
        repo,
        &meta.task_id,
        &clean_author,
        TaskStatus::InProgress,
        &initial_summary,
        &[],
    )?;
    Ok(meta)
}

fn next_sequential_task_id(repo: &Path) -> Result<String> {
    let root = tasks_root(repo);
    let mut max_num = 0u32;
    if root.is_dir() {
        let rd = fs::read_dir(&root).with_context(|| format!("failed to read {}", root.display()))?;
        for entry in rd.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && let Some(num) = parse_short_seq_id(name)
            {
                max_num = max_num.max(num);
            }
        }
    }
    let mut next = max_num.saturating_add(1).max(1);
    while task_dir(repo, &format!("T-{next:03}")).exists() {
        next = next.saturating_add(1);
    }
    Ok(format!("T-{next:03}"))
}

fn parse_short_seq_id(id: &str) -> Option<u32> {
    let digits = id.strip_prefix("T-")?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse::<u32>().ok())
        .flatten()
}

fn sanitize_explicit_id(raw: &str) -> Result<String> {
    let clean: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = clean.trim_matches('-');
    if trimmed.is_empty() {
        bail!("task ID `{raw}` must contain alphanumeric characters");
    }
    Ok(trimmed.to_owned())
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
    append_entry_with_evidence(repo, task_id, author, status, summary, None, files_touched)
}

/// Appends an immutable entry with optional verification `evidence`.
pub fn append_entry_with_evidence(
    repo: &Path,
    task_id: &str,
    author: &str,
    status: TaskStatus,
    summary: &str,
    evidence: Option<&str>,
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
        evidence: evidence.map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned),
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
    atomic_write(&plan_path, markdown.as_bytes())?;
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
    metas.sort_by(|a, b| compare_task_ids(&a.task_id, &b.task_id));
    Ok(metas)
}

/// Renames legacy `T-YYYYMMDD-<ULID>` task folders chronologically to short codes (`T-001`, ...).
pub fn migrate_legacy_task_ids(repo: &Path) -> Result<Vec<(String, String)>> {
    let root = tasks_root(repo);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let lock_path = repo.join(".harness/.state/tasks-id.lock");
    let renamed = with_file_lock(&lock_path, ID_LOCK_TIMEOUT_MS, || {
        let mut legacy = collect_legacy_metas(&root)?;
        legacy.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.task_id.cmp(&b.task_id)));
        let mut pairs = Vec::with_capacity(legacy.len());
        for meta in legacy {
            let new_id = migrate_one_legacy_dir(repo, &meta)?;
            pairs.push((meta.task_id, new_id));
        }
        Ok(pairs)
    })?;
    if !renamed.is_empty() {
        let _ = crate::builder::build_root_tasks_json(repo)?;
    }
    Ok(renamed)
}

fn collect_legacy_metas(root: &Path) -> Result<Vec<TaskMeta>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))?.flatten() {
        let dir = entry.path();
        if !dir.is_dir() || !dir.join("meta.json").is_file() {
            continue;
        }
        if entry.file_name().to_str().is_some_and(is_legacy_task_id) {
            out.push(read_json_file::<TaskMeta>(&dir.join("meta.json"))?);
        }
    }
    Ok(out)
}

fn is_legacy_task_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("T-") else {
        return false;
    };
    let Some((date, ulid)) = rest.split_once('-') else {
        return false;
    };
    date.len() == 8
        && date.bytes().all(|b| b.is_ascii_digit())
        && ulid.len() >= 8
        && ulid.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn migrate_one_legacy_dir(repo: &Path, meta: &TaskMeta) -> Result<String> {
    let new_id = next_sequential_task_id(repo)?;
    let old_dir = task_dir(repo, &meta.task_id);
    let mut updated_meta = meta.clone();
    updated_meta.task_id = new_id.clone();
    atomic_write_json(&old_dir.join("meta.json"), &updated_meta)?;

    let entries_dir = old_dir.join("entries");
    if entries_dir.is_dir() {
        for item in fs::read_dir(&entries_dir)?.flatten() {
            let p = item.path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                let mut e: TaskEntry = read_json_file(&p)?;
                e.task_id = new_id.clone();
                atomic_write_json(&p, &e)?;
            }
        }
    }
    fs::rename(&old_dir, task_dir(repo, &new_id))
        .with_context(|| format!("failed to rename {} -> {new_id}", old_dir.display()))?;

    if let Ok(Some(mut active)) = load_active_task(repo)
        && active.task_id == meta.task_id
    {
        active.task_id = new_id.clone();
        let _ = save_active_task(repo, &active);
    }
    Ok(new_id)
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
