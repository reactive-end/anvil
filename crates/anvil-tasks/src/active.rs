//! Ephemeral per-developer active-task state in `.harness/.state/active-task.json`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use anvil_core::fs_util::with_file_lock;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::builder::build_root_tasks_json;
use crate::curate::{clear_untracked_edits, is_conversational_or_trivial_prompt};
use crate::identity::resolve_author_fast;
use crate::ledger::{start_task_as, task_dir};
use crate::model::{TaskMeta, TaskStatus};
use crate::{atomic_write_json, read_json_file};

pub(crate) const ACTIVE_TASK_LOCK_TIMEOUT_MS: u64 = 2000;

/// Local ephemeral tracking state stored at `.harness/.state/active-task.json` (gitignored).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveTaskState {
    pub task_id: String,
    pub prompt_hash: String,
    pub status: TaskStatus,
    pub summary_recorded_at: Option<DateTime<Utc>>,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub touched_files: Vec<String>,
    #[serde(default)]
    pub skipped: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_progress_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_command_passed: Option<bool>,
}

/// Ephemeral prompt snapshot stored in `.harness/.state/pending-prompt.json` (gitignored)
/// so read-only turns never pollute `.harness/tasks/` or `tasks.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingPrompt {
    pub prompt: String,
    pub prompt_hash: String,
    pub recorded_at: DateTime<Utc>,
}

/// Returns the canonical path `.harness/.state/pending-prompt.json`.
pub fn pending_prompt_path(repo: &Path) -> PathBuf {
    repo.join(".harness").join(".state").join("pending-prompt.json")
}

/// Returns the canonical path `.harness/.state/active-task.json`.
pub fn active_task_path(repo: &Path) -> PathBuf {
    repo.join(".harness").join(".state").join("active-task.json")
}

/// Returns the advisory lock path `.harness/.state/active-task.lock`.
pub fn active_task_lock_path(repo: &Path) -> PathBuf {
    repo.join(".harness").join(".state").join("active-task.lock")
}

/// Loads `.harness/.state/active-task.json` if present and non-empty.
pub fn load_active_task(repo: &Path) -> Result<Option<ActiveTaskState>> {
    load_active_task_unlocked(repo)
}

/// Atomically persists `ActiveTaskState` to `.harness/.state/active-task.json` under lock.
pub fn save_active_task(repo: &Path, state: &ActiveTaskState) -> Result<()> {
    with_file_lock(
        &active_task_lock_path(repo),
        ACTIVE_TASK_LOCK_TIMEOUT_MS,
        || save_active_task_unlocked(repo, state),
    )
}

/// Ensures an active `InProgress` task exists for `prompt`, creating one only when needed.
///
/// Serializes read-modify-write via `.harness/.state/active-task.lock` (2000ms timeout) and uses
/// `resolve_author_fast` so concurrent hooks never drop state or perform live network calls.
/// Returns `Ok(None)` for empty prompts, slash commands (`/...`), or trivial acks (`<= 12` chars).
/// Returns `Ok(Some((state, created)))` where `created` is `true` if a new task was started.
pub fn ensure_task_for_prompt(
    repo: &Path,
    prompt: &str,
) -> Result<Option<(ActiveTaskState, bool)>> {
    let trimmed = prompt.trim();
    if is_ignored_prompt(trimmed) {
        return Ok(None);
    }

    with_file_lock(
        &active_task_lock_path(repo),
        ACTIVE_TASK_LOCK_TIMEOUT_MS,
        || {
            let prompt_hash = hash_prompt(trimmed);
            if let Some(existing) = load_active_task_unlocked(repo)?
                && task_dir(repo, &existing.task_id).join("meta.json").exists()
                && (existing.prompt_hash == prompt_hash || existing.status == TaskStatus::InProgress)
            {
                return Ok(Some((existing, false)));
            }

            let author = resolve_author_fast(repo)?;
            let meta = start_task_as(repo, &author, trimmed)?;
            build_root_tasks_json(repo)?;
            let state = new_active_state(meta.task_id, prompt_hash, meta.created_at, Vec::new());
            save_active_task_unlocked(repo, &state)?;
            Ok(Some((state, true)))
        },
    )
}

/// Records a newly started manual task in `active-task.json` with `summary_recorded_at = None`.
pub fn activate_task_from_meta(repo: &Path, meta: &TaskMeta) -> Result<()> {
    with_file_lock(
        &active_task_lock_path(repo),
        ACTIVE_TASK_LOCK_TIMEOUT_MS,
        || {
            let prompt_hash = load_pending_prompt_unlocked(repo)?
                .map_or_else(|| hash_prompt(meta.request.trim()), |p| p.prompt_hash);
            let state =
                new_active_state(meta.task_id.clone(), prompt_hash, meta.created_at, Vec::new());
            save_active_task_unlocked(repo, &state)?;
            let _ = fs::remove_file(pending_prompt_path(repo));
            clear_untracked_edits(repo);
            Ok(())
        },
    )
}

pub(crate) fn new_active_state(
    task_id: String,
    prompt_hash: String,
    started_at: DateTime<Utc>,
    touched_files: Vec<String>,
) -> ActiveTaskState {
    ActiveTaskState {
        task_id,
        prompt_hash,
        status: TaskStatus::InProgress,
        summary_recorded_at: None,
        started_at,
        touched_files,
        skipped: false,
        skip_reason: None,
        last_progress_at: Some(started_at),
        last_summary: None,
        next_steps: Vec::new(),
        last_command: None,
        last_command_passed: None,
    }
}

/// Captures a non-trivial user prompt in `.harness/.state/pending-prompt.json` without creating
/// a task on disk yet. A task is only materialized lazily on the first mutating file edit.
pub fn record_pending_prompt(repo: &Path, prompt: &str) -> Result<bool> {
    let trimmed = prompt.trim();
    if is_ignored_prompt(trimmed) {
        return Ok(false);
    }
    with_file_lock(
        &active_task_lock_path(repo),
        ACTIVE_TASK_LOCK_TIMEOUT_MS,
        || {
            let prompt_hash = hash_prompt(trimmed);
            if let Some(existing) = load_active_task_unlocked(repo)?
                && (existing.skipped || task_dir(repo, &existing.task_id).join("meta.json").exists())
                && (existing.prompt_hash == prompt_hash || existing.status == TaskStatus::InProgress)
            {
                return Ok(false);
            }
            let pending = PendingPrompt {
                prompt: trimmed.to_owned(),
                prompt_hash,
                recorded_at: Utc::now(),
            };
            atomic_write_json(&pending_prompt_path(repo), &pending)?;
            Ok(true)
        },
    )
}

/// Lazily materializes a tracked task on the first mutated project file (consuming
/// `pending-prompt.json`), or appends `rel_path` if an `InProgress` task is already active.
/// Returns `Ok(Some(state))` ONLY when a new task was created on this edit.
pub fn materialize_task_on_edit(repo: &Path, rel_path: &str) -> Result<Option<ActiveTaskState>> {
    materialize_task_on_edit_with_fallback(repo, rel_path, None)
}

/// Same as [`materialize_task_on_edit`], falling back to `fallback_prompt_root`'s recent
/// `pending-prompt.json` when edits originate from a parent multi-repo workspace.
pub fn materialize_task_on_edit_with_fallback(
    repo: &Path,
    rel_path: &str,
    fallback_prompt_root: Option<&Path>,
) -> Result<Option<ActiveTaskState>> {
    let norm = rel_path.trim().replace('\\', "/");
    let norm = norm.strip_prefix("./").unwrap_or(&norm);
    if norm.is_empty() || norm.starts_with(".harness/") || norm == "tasks.json" {
        return Ok(None);
    }
    with_file_lock(
        &active_task_lock_path(repo),
        ACTIVE_TASK_LOCK_TIMEOUT_MS,
        || {
            if let Some(mut existing) = load_active_task_unlocked(repo)?
                && task_dir(repo, &existing.task_id).join("meta.json").exists()
                && existing.status == TaskStatus::InProgress
            {
                if !existing.touched_files.iter().any(|f| f == norm) {
                    existing.touched_files.push(norm.to_owned());
                    existing.touched_files.sort();
                    save_active_task_unlocked(repo, &existing)?;
                }
                return Ok(None);
            }

            let pending = load_pending_prompt_unlocked(repo)?
                .or_else(|| load_fresh_fallback_prompt(fallback_prompt_root));
            let (request, prompt_hash) = match pending {
                Some(p) => (p.prompt, p.prompt_hash),
                None => {
                    let fallback = format!("Edit {norm}");
                    let hash = hash_prompt(&fallback);
                    (fallback, hash)
                }
            };
            let author = resolve_author_fast(repo)?;
            let meta = start_task_as(repo, &author, &request)?;
            build_root_tasks_json(repo)?;
            let state =
                new_active_state(meta.task_id, prompt_hash, meta.created_at, vec![norm.to_owned()]);
            save_active_task_unlocked(repo, &state)?;
            let _ = fs::remove_file(pending_prompt_path(repo));
            Ok(Some(state))
        },
    )
}

fn load_fresh_fallback_prompt(fallback_root: Option<&Path>) -> Option<PendingPrompt> {
    let root = fallback_root?;
    let p = load_pending_prompt_unlocked(root).ok().flatten()?;
    let age_secs = (Utc::now() - p.recorded_at).num_seconds();
    (age_secs <= 600).then_some(p)
}

/// Appends a mutated repo-relative file path to the active `InProgress` task under lock.
pub fn record_touched_file(repo: &Path, rel_path: &str) -> Result<()> {
    let norm = rel_path.trim().replace('\\', "/");
    let norm = norm.strip_prefix("./").unwrap_or(&norm);
    if norm.is_empty() || norm.starts_with(".harness/") || norm == "tasks.json" {
        return Ok(());
    }
    with_file_lock(
        &active_task_lock_path(repo),
        ACTIVE_TASK_LOCK_TIMEOUT_MS,
        || {
            let Some(mut state) = load_active_task_unlocked(repo)? else {
                return Ok(());
            };
            if state.status != TaskStatus::InProgress {
                return Ok(());
            }
            if !state.touched_files.iter().any(|f| f == norm) {
                state.touched_files.push(norm.to_owned());
                state.touched_files.sort();
                save_active_task_unlocked(repo, &state)?;
            }
            Ok(())
        },
    )
}

/// Updates `.harness/.state/active-task.json` under lock when a summary is recorded.
pub fn mark_summary_recorded(repo: &Path, task_id: &str, status: TaskStatus) -> Result<()> {
    mark_summary_recorded_with_files(repo, task_id, status, &[])
}

/// Updates `.harness/.state/active-task.json` under lock, also syncing `files_touched`.
pub fn mark_summary_recorded_with_files(
    repo: &Path,
    task_id: &str,
    status: TaskStatus,
    files: &[String],
) -> Result<()> {
    with_file_lock(
        &active_task_lock_path(repo),
        ACTIVE_TASK_LOCK_TIMEOUT_MS,
        || upsert_active_state_unlocked(repo, task_id, status, None, None, files, true),
    )
}

/// Updates `.harness/.state/active-task.json` with incremental progress or final completion.
/// Keeps `summary_recorded_at = None` while `InProgress` so unfinished sessions can resume cleanly.
pub fn update_active_task_progress(
    repo: &Path,
    task_id: &str,
    status: TaskStatus,
    summary: &str,
    next_steps: &[String],
    files: &[String],
) -> Result<()> {
    with_file_lock(
        &active_task_lock_path(repo),
        ACTIVE_TASK_LOCK_TIMEOUT_MS,
        || {
            upsert_active_state_unlocked(
                repo,
                task_id,
                status,
                Some(summary),
                Some(next_steps),
                files,
                status == TaskStatus::Done,
            )
        },
    )
}

/// Records the latest build/test command outcome onto the currently active `InProgress` task.
pub fn record_active_command_outcome(repo: &Path, command: &str, passed: bool) -> Result<()> {
    let clean = command.trim();
    if clean.is_empty() {
        return Ok(());
    }
    with_file_lock(
        &active_task_lock_path(repo),
        ACTIVE_TASK_LOCK_TIMEOUT_MS,
        || {
            let Some(mut state) = load_active_task_unlocked(repo)? else {
                return Ok(());
            };
            if state.skipped || state.status != TaskStatus::InProgress {
                return Ok(());
            }
            state.last_command = Some(clean.to_owned());
            state.last_command_passed = Some(passed);
            state.last_progress_at = Some(Utc::now());
            save_active_task_unlocked(repo, &state)
        },
    )
}

fn upsert_active_state_unlocked(
    repo: &Path,
    task_id: &str,
    status: TaskStatus,
    summary: Option<&str>,
    next_steps: Option<&[String]>,
    files: &[String],
    mark_summary: bool,
) -> Result<()> {
    let now = Utc::now();
    let mut next = load_or_seed_active_state(repo, task_id, status, now)?;
    next.status = status;
    next.summary_recorded_at = if mark_summary { Some(now) } else { None };
    next.skipped = false;
    next.skip_reason = None;
    next.last_progress_at = Some(now);
    if let Some(text) = summary {
        next.last_summary = Some(text.to_owned());
    }
    if let Some(steps) = next_steps {
        next.next_steps = steps.to_vec();
    }
    merge_active_files(&mut next.touched_files, files);
    if let Ok(Some(pending)) = load_pending_prompt_unlocked(repo) {
        next.prompt_hash = pending.prompt_hash;
    }
    save_active_task_unlocked(repo, &next)?;
    let _ = fs::remove_file(pending_prompt_path(repo));
    clear_untracked_edits(repo);
    Ok(())
}

fn load_or_seed_active_state(
    repo: &Path,
    task_id: &str,
    status: TaskStatus,
    now: DateTime<Utc>,
) -> Result<ActiveTaskState> {
    if let Some(existing) = load_active_task_unlocked(repo)?
        && existing.task_id == task_id
    {
        return Ok(existing);
    }
    let meta_path = task_dir(repo, task_id).join("meta.json");
    let (prompt_hash, started_at) = if meta_path.exists() {
        let meta: TaskMeta = read_json_file(&meta_path)?;
        (hash_prompt(meta.request.trim()), meta.created_at)
    } else {
        (String::new(), now)
    };
    let mut seeded = new_active_state(task_id.to_owned(), prompt_hash, started_at, Vec::new());
    seeded.status = status;
    Ok(seeded)
}

fn merge_active_files(dst: &mut Vec<String>, files: &[String]) {
    for f in files {
        let norm = f.trim().replace('\\', "/");
        if !norm.is_empty() && !dst.iter().any(|x| x == &norm) {
            dst.push(norm);
        }
    }
    dst.sort();
}

pub(crate) fn load_pending_prompt_unlocked(repo: &Path) -> Result<Option<PendingPrompt>> {
    let path = pending_prompt_path(repo);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let pending: PendingPrompt = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse JSON at {}", path.display()))?;
    Ok(Some(pending))
}

pub(crate) fn load_active_task_unlocked(repo: &Path) -> Result<Option<ActiveTaskState>> {
    let path = active_task_path(repo);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let state: ActiveTaskState = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse JSON at {}", path.display()))?;
    Ok(Some(state))
}

pub(crate) fn save_active_task_unlocked(repo: &Path, state: &ActiveTaskState) -> Result<()> {
    atomic_write_json(&active_task_path(repo), state)
}

fn is_ignored_prompt(trimmed: &str) -> bool {
    is_conversational_or_trivial_prompt(trimmed)
}

pub(crate) fn hash_prompt(trimmed: &str) -> String {
    blake3::hash(trimmed.as_bytes()).to_hex().to_string()
}
