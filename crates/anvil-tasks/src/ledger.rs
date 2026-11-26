//! Conflict-free task ledger operations on `.harness/tasks/<id>/`.

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rustc_hash::FxHashSet;
use ulid::Ulid;

use anvil_core::fs_util::{atomic_write, sanitize_author, with_file_lock};

use crate::identity::resolve_author;
use crate::model::{TaskEntry, TaskMeta, TaskStatus};
use crate::progress::{ProgressUpdate, write_task_progress_md};
use crate::{atomic_write_json, read_json_file};

const ID_LOCK_TIMEOUT_MS: u64 = 5000;

/// Returns `.harness/tasks`.
pub fn tasks_root(repo: &Path) -> PathBuf {
    repo.join(".harness").join("tasks")
}

/// Returns `.harness/tasks/<task_id>`, supporting flat (`T-001`) and 1-level grouped (`voz/F001`) IDs.
pub fn task_dir(repo: &Path, task_id: &str) -> PathBuf {
    let mut path = tasks_root(repo);
    for seg in task_id.replace('\\', "/").split('/') {
        if !seg.is_empty() && seg != "." && seg != ".." {
            path.push(seg);
        }
    }
    path
}

/// Sanitizes a task group subdirectory name (`voz`, `render-voz`, etc.), rejecting traversal/reserved names.
pub fn sanitize_group_name(raw: &str) -> Result<String> {
    let trimmed_raw = raw.trim();
    if trimmed_raw.contains("..") || trimmed_raw.contains('/') || trimmed_raw.contains('\\') {
        bail!("group name `{raw}` must be a single folder name without path separators or `..`");
    }
    let clean: String = trimmed_raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '-' })
        .collect();
    let trimmed = clean.trim_matches('-');
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("entries") {
        bail!("invalid task group name `{raw}`");
    }
    Ok(trimmed.to_owned())
}

/// Validates a flat (`F001`) or 1-level grouped (`voz/F001`) task ID against path traversal.
pub fn validate_task_id_path(task_id: &str) -> Result<String> {
    let norm = task_id.trim().replace('\\', "/");
    if norm.is_empty() || norm.starts_with('/') || norm.ends_with('/') {
        bail!("invalid task ID `{task_id}`");
    }
    let parts: Vec<&str> = norm.split('/').collect();
    if parts.len() > 2 {
        bail!("task ID `{task_id}` may have at most one group prefix (`group/id`)");
    }
    for (idx, seg) in parts.iter().enumerate() {
        let valid_chars = !seg.is_empty()
            && *seg != "."
            && *seg != ".."
            && seg.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
        if !valid_chars || (idx == 0 && parts.len() == 2 && seg.eq_ignore_ascii_case("entries")) {
            bail!("invalid task ID segment `{seg}` in `{task_id}`");
        }
    }
    Ok(parts.join("/"))
}

/// Creates a new task directory, writes `meta.json`, and records the initial entry.
pub fn start_task(repo: &Path, request: &str) -> Result<TaskMeta> {
    let author = resolve_author(repo)?;
    start_task_as(repo, &author, request)
}

/// Compares two task IDs naturally across optional groups (`T-002 < T-010`, `voz/F002 < voz/F038`).
pub fn compare_task_ids(a: &str, b: &str) -> Ordering {
    let (ga, ca) = split_group_and_code(a);
    let (gb, cb) = split_group_and_code(b);
    ga.cmp(&gb)
        .then_with(|| {
            let (pa, na) = split_id_prefix_num(ca);
            let (pb, nb) = split_id_prefix_num(cb);
            pa.cmp(pb).then_with(|| match (na, nb) {
                (Some(x), Some(y)) => x.cmp(&y),
                (None, Some(_)) => Ordering::Less,
                (Some(_), None) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            })
        })
        .then_with(|| a.cmp(b))
}

fn split_group_and_code(id: &str) -> (Option<&str>, &str) {
    match id.split_once('/') {
        Some((g, code)) => (Some(g), code),
        None => (None, id),
    }
}

fn split_id_prefix_num(id: &str) -> (&str, Option<u64>) {
    let digit_start = id.rfind(|c: char| !c.is_ascii_digit()).map_or(0, |i| i + 1);
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
    start_task_in_group(repo, author, request, None, explicit_id)
}

/// Creates a new task in an optional subdirectory group (`voz`) with an optional explicit ID.
pub fn start_task_in_group(
    repo: &Path,
    author: &str,
    request: &str,
    group: Option<&str>,
    explicit_id: Option<&str>,
) -> Result<TaskMeta> {
    let clean_author = sanitize_author(author);
    let meta = create_task_meta_in_group(repo, &clean_author, request, group, explicit_id)?;
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

/// Allocates and persists a task `meta.json` under `.harness/tasks/` without writing an initial entry.
pub(crate) fn create_task_meta_in_group(
    repo: &Path,
    author: &str,
    request: &str,
    group: Option<&str>,
    explicit_id: Option<&str>,
) -> Result<TaskMeta> {
    let clean_author = sanitize_author(author);
    let clean_group = group.map(sanitize_group_name).transpose()?;
    let now = Utc::now();
    let lock_path = repo.join(".harness/.state/tasks-id.lock");

    with_file_lock(&lock_path, ID_LOCK_TIMEOUT_MS, || {
        let task_id = resolve_new_task_id(repo, clean_group.as_deref(), explicit_id)?;
        let dir = task_dir(repo, &task_id);
        fs::create_dir_all(dir.join("entries"))
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let meta = TaskMeta {
            task_id,
            created_by: clean_author.clone(),
            request: request.trim().to_owned(),
            project: clean_group.clone(),
            behavior: None,
            verify: None,
            created_at: now,
        };
        atomic_write_json(&dir.join("meta.json"), &meta)?;
        Ok(meta)
    })
}

fn resolve_new_task_id(
    repo: &Path,
    group: Option<&str>,
    explicit_id: Option<&str>,
) -> Result<String> {
    match explicit_id {
        Some(raw) => {
            let clean = sanitize_explicit_id(raw)?;
            let full = match group {
                Some(g) => format!("{g}/{clean}"),
                None => clean,
            };
            let valid = validate_task_id_path(&full)?;
            if task_dir(repo, &valid).join("meta.json").exists() {
                bail!("task `{valid}` already exists");
            }
            Ok(valid)
        }
        None => next_sequential_task_id_in(repo, group),
    }
}

fn next_sequential_task_id(repo: &Path) -> Result<String> {
    next_sequential_task_id_in(repo, None)
}

fn next_sequential_task_id_in(repo: &Path, group: Option<&str>) -> Result<String> {
    let scan_dir = match group {
        Some(g) => tasks_root(repo).join(g),
        None => tasks_root(repo),
    };
    let mut max_num = 0u32;
    if scan_dir.is_dir() {
        let rd = fs::read_dir(&scan_dir)
            .with_context(|| format!("failed to read {}", scan_dir.display()))?;
        for entry in rd.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && let Some(num) = parse_short_seq_id(name)
            {
                max_num = max_num.max(num);
            }
        }
    }
    let prefix = group.map_or_else(String::new, |g| format!("{g}/"));
    let mut next = max_num.saturating_add(1).max(1);
    while task_dir(repo, &format!("{prefix}T-{next:03}")).exists() {
        next = next.saturating_add(1);
    }
    Ok(format!("{prefix}T-{next:03}"))
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
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '-' })
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
    append_entry_with_details(
        repo,
        task_id,
        author,
        &ProgressUpdate {
            status,
            summary,
            done_steps: &[],
            next_steps: &[],
            blocker: None,
            evidence,
            files: files_touched,
        },
    )
}

/// Appends an immutable entry with structured steps/blocker/evidence and refreshes `progress.md`.
pub fn append_entry_with_details(
    repo: &Path,
    task_id: &str,
    author: &str,
    details: &ProgressUpdate<'_>,
) -> Result<TaskEntry> {
    let valid_id = validate_task_id_path(task_id)?;
    let dir = task_dir(repo, &valid_id);
    let meta_path = dir.join("meta.json");
    if !meta_path.exists() {
        bail!("task `{valid_id}` not found at {}", meta_path.display());
    }

    let clean_author = sanitize_author(author);
    let entry_id = Ulid::new().to_string();
    let branch = anvil_core::git::current_branch(repo).unwrap_or_else(|_| "HEAD".to_owned());
    let entry = TaskEntry {
        entry_id: entry_id.clone(),
        task_id: valid_id.clone(),
        author: clean_author.clone(),
        status: details.status,
        branch,
        summary: details.summary.trim().to_owned(),
        evidence: opt_trimmed(details.evidence),
        files_touched: normalize_files(details.files),
        done_steps: normalize_steps(details.done_steps),
        next_steps: normalize_steps(details.next_steps),
        blocker: opt_trimmed(details.blocker),
        timestamp: Utc::now(),
    };

    let entry_path = dir.join("entries").join(format!("{clean_author}-{entry_id}.json"));
    if entry_path.exists() {
        bail!("immutable entry already exists at {}", entry_path.display());
    }
    atomic_write_json(&entry_path, &entry)?;
    write_task_progress_md(repo, &valid_id)?;
    Ok(entry)
}

fn opt_trimmed(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn normalize_steps(steps: &[String]) -> Vec<String> {
    let mut seen = FxHashSet::default();
    let mut out = Vec::new();
    for raw in steps {
        let trimmed = raw.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_owned()) {
            out.push(trimmed.to_owned());
        }
    }
    out
}

/// Writes an isolated per-author plan file `.harness/tasks/<id>/plan-<author>.md`.
pub fn write_task_plan(
    repo: &Path,
    task_id: &str,
    author: &str,
    markdown: &str,
) -> Result<PathBuf> {
    let valid_id = validate_task_id_path(task_id)?;
    let dir = task_dir(repo, &valid_id);
    if !dir.join("meta.json").exists() {
        bail!("task `{valid_id}` not found at {}", dir.display());
    }
    let clean_author = sanitize_author(author);
    let plan_path = dir.join(format!("plan-{clean_author}.md"));
    atomic_write(&plan_path, markdown.as_bytes())?;
    Ok(plan_path)
}

/// Discovers flat (depth-1) and grouped (depth-2) task directories under `.harness/tasks/`.
pub(crate) fn discover_task_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut dirs = Vec::new();
    for entry in
        fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))?.flatten()
    {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        if p.join("meta.json").is_file() {
            dirs.push(p);
            continue;
        }
        if let Ok(sub_rd) = fs::read_dir(&p) {
            for sub in sub_rd.flatten() {
                let sp = sub.path();
                if sp.is_dir() && sp.join("meta.json").is_file() {
                    dirs.push(sp);
                }
            }
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Lists sorted subdirectory group names under `.harness/tasks/` that hold grouped tasks.
pub fn list_task_groups(repo: &Path) -> Vec<String> {
    let root = tasks_root(repo);
    let Ok(rd) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut groups = Vec::new();
    for entry in rd.flatten() {
        let p = entry.path();
        if !p.is_dir() || p.join("meta.json").is_file() {
            continue;
        }
        let has_tasks = fs::read_dir(&p)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .any(|s| s.path().is_dir() && s.path().join("meta.json").is_file());
        if has_tasks && let Some(name) = entry.file_name().to_str() {
            groups.push(name.to_owned());
        }
    }
    groups.sort();
    groups
}

/// Lists all task metadata records under `.harness/tasks/`, sorted naturally by `task_id`.
pub fn list_tasks(repo: &Path) -> Result<Vec<TaskMeta>> {
    let dirs = discover_task_dirs(&tasks_root(repo))?;
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
        legacy.sort_by(|a, b| {
            a.created_at.cmp(&b.created_at).then_with(|| a.task_id.cmp(&b.task_id))
        });
        let mut pairs = Vec::with_capacity(legacy.len());
        for meta in legacy {
            let new_id = migrate_one_legacy_dir(repo, &meta)?;
            pairs.push((meta.task_id, new_id));
        }
        Ok(pairs)
    })?;
    if !renamed.is_empty() {
        update_active_task_after_migration(repo, &renamed);
        let _ = crate::builder::build_root_tasks_json(repo)?;
    }
    Ok(renamed)
}

fn update_active_task_after_migration(repo: &Path, renamed: &[(String, String)]) {
    let lock = crate::active::active_task_lock_path(repo);
    let _ = with_file_lock(&lock, 2_000, || {
        if let Ok(Some(mut active)) = crate::active::load_active_task_unlocked(repo)
            && let Some((_, new_id)) = renamed.iter().find(|(old, _)| old == &active.task_id)
        {
            active.task_id = new_id.clone();
            let _ = crate::active::save_active_task_unlocked(repo, &active);
        }
        Ok(())
    });
}

fn collect_legacy_metas(root: &Path) -> Result<Vec<TaskMeta>> {
    let mut out = Vec::new();
    for entry in
        fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))?.flatten()
    {
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
    let _ = write_task_progress_md(repo, &new_id);
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
    if iter.next().is_some() { format!("{taken}...") } else { taken }
}
