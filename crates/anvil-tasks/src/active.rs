//! Ephemeral per-developer active-task state in `.harness/.state/active-task.json`.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use anvil_core::fs_util::with_file_lock;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::builder::build_root_tasks_json;
use crate::identity::resolve_author_fast;
use crate::ledger::{start_task_as, task_dir};
use crate::model::{TaskMeta, TaskStatus};
use crate::{atomic_write_json, read_json_file};

const TRIVIAL_PROMPT_MAX_CHARS: usize = 12;
const ACTIVE_TASK_LOCK_TIMEOUT_MS: u64 = 2000;

/// Local ephemeral tracking state stored at `.harness/.state/active-task.json` (gitignored).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveTaskState {
    pub task_id: String,
    pub prompt_hash: String,
    pub status: TaskStatus,
    pub summary_recorded_at: Option<DateTime<Utc>>,
    pub started_at: DateTime<Utc>,
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
    with_active_task_lock(repo, || save_active_task_unlocked(repo, state))
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

    with_active_task_lock(repo, || {
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
        let state = ActiveTaskState {
            task_id: meta.task_id,
            prompt_hash,
            status: TaskStatus::InProgress,
            summary_recorded_at: None,
            started_at: meta.created_at,
        };
        save_active_task_unlocked(repo, &state)?;
        Ok(Some((state, true)))
    })
}

/// Records a newly started manual task in `active-task.json` with `summary_recorded_at = None`.
pub fn activate_task_from_meta(repo: &Path, meta: &TaskMeta) -> Result<()> {
    let state = ActiveTaskState {
        task_id: meta.task_id.clone(),
        prompt_hash: hash_prompt(meta.request.trim()),
        status: TaskStatus::InProgress,
        summary_recorded_at: None,
        started_at: meta.created_at,
    };
    save_active_task(repo, &state)
}

/// Updates `.harness/.state/active-task.json` under lock when a summary is recorded.
pub fn mark_summary_recorded(repo: &Path, task_id: &str, status: TaskStatus) -> Result<()> {
    with_active_task_lock(repo, || {
        let now = Utc::now();
        let next = match load_active_task_unlocked(repo)? {
            Some(mut existing) if existing.task_id == task_id => {
                existing.status = status;
                existing.summary_recorded_at = Some(now);
                existing
            }
            _ => {
                let meta_path = task_dir(repo, task_id).join("meta.json");
                let (prompt_hash, started_at) = if meta_path.exists() {
                    let meta: TaskMeta = read_json_file(&meta_path)?;
                    (hash_prompt(meta.request.trim()), meta.created_at)
                } else {
                    (String::new(), now)
                };
                ActiveTaskState {
                    task_id: task_id.to_owned(),
                    prompt_hash,
                    status,
                    summary_recorded_at: Some(now),
                    started_at,
                }
            }
        };
        save_active_task_unlocked(repo, &next)
    })
}

fn with_active_task_lock<T, F: FnOnce() -> Result<T>>(repo: &Path, f: F) -> Result<T> {
    let lock_path = active_task_lock_path(repo);
    let deadline = Instant::now() + Duration::from_millis(ACTIVE_TASK_LOCK_TIMEOUT_MS);
    let mut f_opt = Some(f);
    let mut backoff_ms = 2u64;

    loop {
        let remaining_ms = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .max(1) as u64;
        let res = with_file_lock(&lock_path, remaining_ms, || {
            let func = f_opt
                .take()
                .ok_or_else(|| anyhow!("active-task lock closure already consumed"))?;
            func()
        });
        match res {
            Ok(val) => return Ok(val),
            // On Windows NTFS, CreateFile(CREATE_NEW) against a delete-pending lock returns PermissionDenied.
            Err(err) if f_opt.is_some() && is_windows_delete_pending_err(&err) => {
                let now = Instant::now();
                if now >= deadline {
                    return Err(err);
                }
                thread::sleep(Duration::from_millis(backoff_ms).min(deadline - now));
                backoff_ms = (backoff_ms * 2).min(25);
            }
            Err(err) => return Err(err),
        }
    }
}

fn is_windows_delete_pending_err(err: &anyhow::Error) -> bool {
    cfg!(windows)
        && err.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io_err| io_err.kind() == std::io::ErrorKind::PermissionDenied)
        })
}

fn load_active_task_unlocked(repo: &Path) -> Result<Option<ActiveTaskState>> {
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

fn save_active_task_unlocked(repo: &Path, state: &ActiveTaskState) -> Result<()> {
    atomic_write_json(&active_task_path(repo), state)
}

fn is_ignored_prompt(trimmed: &str) -> bool {
    trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.chars().count() <= TRIVIAL_PROMPT_MAX_CHARS
}

fn hash_prompt(trimmed: &str) -> String {
    blake3::hash(trimmed.as_bytes()).to_hex().to_string()
}
