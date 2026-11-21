//! Ephemeral per-developer active-task state in `.harness/.state/active-task.json`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::builder::build_root_tasks_json;
use crate::fs_util::{atomic_write_json, read_json_file};
use crate::ledger::{start_task, task_dir};
use crate::model::{TaskMeta, TaskStatus};

const TRIVIAL_PROMPT_MAX_CHARS: usize = 12;

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

/// Loads `.harness/.state/active-task.json` if present and non-empty.
pub fn load_active_task(repo: &Path) -> Result<Option<ActiveTaskState>> {
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

/// Atomically persists `ActiveTaskState` to `.harness/.state/active-task.json`.
pub fn save_active_task(repo: &Path, state: &ActiveTaskState) -> Result<()> {
    atomic_write_json(&active_task_path(repo), state)
}

/// Ensures an active `InProgress` task exists for `prompt`, creating one only when needed.
///
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

    let prompt_hash = hash_prompt(trimmed);
    if let Some(existing) = load_active_task(repo)?
        && task_dir(repo, &existing.task_id).join("meta.json").exists()
        && (existing.prompt_hash == prompt_hash || existing.status == TaskStatus::InProgress)
    {
        return Ok(Some((existing, false)));
    }

    let meta = start_task(repo, trimmed)?;
    build_root_tasks_json(repo)?;
    let state = ActiveTaskState {
        task_id: meta.task_id,
        prompt_hash,
        status: TaskStatus::InProgress,
        summary_recorded_at: None,
        started_at: meta.created_at,
    };
    save_active_task(repo, &state)?;
    Ok(Some((state, true)))
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

/// Updates `.harness/.state/active-task.json` when a summary is recorded via `update` or `done`.
pub fn mark_summary_recorded(repo: &Path, task_id: &str, status: TaskStatus) -> Result<()> {
    let now = Utc::now();
    let next = match load_active_task(repo)? {
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
    save_active_task(repo, &next)
}

fn is_ignored_prompt(trimmed: &str) -> bool {
    trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.chars().count() <= TRIVIAL_PROMPT_MAX_CHARS
}

fn hash_prompt(trimmed: &str) -> String {
    blake3::hash(trimmed.as_bytes()).to_hex().to_string()
}
