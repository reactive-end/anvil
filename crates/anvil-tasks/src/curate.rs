//! LLM-curated task governance, conversational noise filtering, and title hygiene validation.

use std::fs;
use std::path::{Path, PathBuf};

use anvil_core::fs_util::with_file_lock;
use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::active::{
    ACTIVE_TASK_LOCK_TIMEOUT_MS, ActiveTaskState, active_task_lock_path, active_task_path,
    hash_prompt, load_active_task_unlocked, load_pending_prompt_unlocked,
    mark_summary_recorded_with_files, pending_prompt_path, save_active_task_unlocked,
};
use crate::builder::build_root_tasks_json;
use crate::files::{collect_task_files, is_project_file};
use crate::identity::{resolve_author, resolve_author_fast};
use crate::ledger::{append_entry_as, create_task_meta_in_group, task_dir};
use crate::model::{TaskEntry, TaskMeta, TaskStatus};
use crate::{atomic_write_json, read_json_file};

const MAX_TITLE_CHARS: usize = 200;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct UntrackedEdits {
    files: Vec<String>,
    first_edited_at: Option<DateTime<Utc>>,
}

/// Returns `.harness/.state/untracked-edits.json` (gitignored).
pub fn untracked_edits_path(repo: &Path) -> PathBuf {
    repo.join(".harness").join(".state").join("untracked-edits.json")
}

/// Returns `true` ONLY for structurally non-task inputs (empty strings or CLI/chat `/` commands).
/// Deciding whether a real request deserves a tracked task belongs to the LLM, not hardcoded phrases.
pub fn is_conversational_or_trivial_prompt(raw: &str) -> bool {
    let trimmed = raw.trim();
    trimmed.is_empty() || trimmed.starts_with('/')
}

/// Validates and normalizes an LLM-curated task title structurally (single non-empty line, <= 200 chars).
pub fn validate_curated_task_title(raw: &str) -> Result<String> {
    if raw.contains('\n') || raw.contains('\r') {
        bail!("task title must be a single concise line");
    }
    let clean = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let char_count = clean.chars().count();
    if clean.is_empty() || char_count > MAX_TITLE_CHARS {
        bail!("rejected task title `{clean}`: must be 1-{MAX_TITLE_CHARS} chars");
    }
    Ok(clean)
}

/// Tracks a mutated project file in `.harness/.state/` without auto-creating a task on disk.
/// Returns `Ok(true)` only on the first untracked edit when no `InProgress` task is active.
pub fn record_session_edit(repo: &Path, rel_path: &str) -> Result<bool> {
    let norm = rel_path.trim().replace('\\', "/");
    let norm = norm.strip_prefix("./").unwrap_or(&norm);
    if !is_project_file(norm) {
        return Ok(false);
    }
    with_file_lock(&active_task_lock_path(repo), ACTIVE_TASK_LOCK_TIMEOUT_MS, || {
        if let Some(mut active) = load_active_task_unlocked(repo)? {
            if active.skipped {
                return Ok(false);
            }
            if active.status == TaskStatus::InProgress
                && task_dir(repo, &active.task_id).join("meta.json").exists()
            {
                if !active.touched_files.iter().any(|f| f == norm) {
                    active.touched_files.push(norm.to_owned());
                    active.touched_files.sort();
                    save_active_task_unlocked(repo, &active)?;
                }
                return Ok(false);
            }
        }

        let path = untracked_edits_path(repo);
        let mut edits = if path.is_file() {
            read_json_file::<UntrackedEdits>(&path).unwrap_or_default()
        } else {
            UntrackedEdits::default()
        };
        let was_empty = edits.files.is_empty();
        if edits.first_edited_at.is_none() {
            edits.first_edited_at = Some(Utc::now());
        }
        if !edits.files.iter().any(|f| f == norm) {
            edits.files.push(norm.to_owned());
            edits.files.sort();
            atomic_write_json(&path, &edits)?;
        }
        Ok(was_empty)
    })
}

/// Loads any untracked session edit paths recorded since the last task decision.
pub fn load_untracked_edits(repo: &Path) -> Vec<String> {
    let path = untracked_edits_path(repo);
    if !path.is_file() {
        return Vec::new();
    }
    read_json_file::<UntrackedEdits>(&path).map(|u| u.files).unwrap_or_default()
}

/// Returns when the oldest untracked session edit happened, if any.
pub fn load_first_untracked_edit_at(repo: &Path) -> Option<DateTime<Utc>> {
    let path = untracked_edits_path(repo);
    read_json_file::<UntrackedEdits>(&path).ok()?.first_edited_at
}

/// Removes `.harness/.state/untracked-edits.json` if present.
pub fn clear_untracked_edits(repo: &Path) {
    let _ = fs::remove_file(untracked_edits_path(repo));
}

/// Clears `.harness/.state/active-task.json` when it holds an ephemeral `skipped` marker from a turn.
pub fn clear_skipped_active_task(repo: &Path) {
    let _ = with_file_lock(&active_task_lock_path(repo), ACTIVE_TASK_LOCK_TIMEOUT_MS, || {
        if load_active_task_unlocked(repo)?.is_some_and(|st| st.skipped) {
            let _ = fs::remove_file(active_task_path(repo));
        }
        Ok(())
    });
}

/// Records an explicit LLM decision to skip task creation for minor tweaks/chores,
/// updating only gitignored `.harness/.state/active-task.json` (`0` entries in `tasks.json`).
pub fn record_task_skip(repo: &Path, reason: &str) -> Result<ActiveTaskState> {
    let clean_reason = reason.trim();
    if clean_reason.is_empty() {
        bail!("skip reason must not be empty");
    }
    with_file_lock(&active_task_lock_path(repo), ACTIVE_TASK_LOCK_TIMEOUT_MS, || {
        let now = Utc::now();
        let prompt_hash = load_pending_prompt_unlocked(repo)?
            .map_or_else(|| hash_prompt(clean_reason), |p| p.prompt_hash);
        let state = ActiveTaskState {
            task_id: "SKIPPED".to_owned(),
            prompt_hash,
            status: TaskStatus::Done,
            summary_recorded_at: Some(now),
            started_at: now,
            touched_files: Vec::new(),
            skipped: true,
            skip_reason: Some(clean_reason.to_owned()),
            last_progress_at: Some(now),
            last_summary: Some(clean_reason.to_owned()),
            next_steps: Vec::new(),
            last_command: None,
            last_command_passed: None,
        };
        save_active_task_unlocked(repo, &state)?;
        let _ = fs::remove_file(pending_prompt_path(repo));
        clear_untracked_edits(repo);
        Ok(state)
    })
}

/// Optional author, group subdirectory, and custom task code for [`record_curated_task_in_group`].
#[derive(Debug, Clone, Copy, Default)]
pub struct TaskTarget<'a> {
    pub author: Option<&'a str>,
    pub group: Option<&'a str>,
    pub task_id: Option<&'a str>,
}

/// Creates and records an LLM-curated task in a single step after validating `title`.
pub fn record_curated_task(
    repo: &Path,
    title: &str,
    summary: &str,
    status: TaskStatus,
    files: &[String],
    author: Option<&str>,
) -> Result<(TaskMeta, TaskEntry)> {
    record_curated_task_in_group(
        repo,
        title,
        summary,
        status,
        files,
        TaskTarget { author, ..TaskTarget::default() },
    )
}

/// Same as [`record_curated_task`] with an optional explicit short task ID (`F039`, `T-042`).
pub fn record_curated_task_with_id(
    repo: &Path,
    title: &str,
    summary: &str,
    status: TaskStatus,
    files: &[String],
    author: Option<&str>,
    explicit_id: Option<&str>,
) -> Result<(TaskMeta, TaskEntry)> {
    record_curated_task_in_group(
        repo,
        title,
        summary,
        status,
        files,
        TaskTarget { author, group: None, task_id: explicit_id },
    )
}

/// Records an LLM-curated task optionally scoped under subdirectory `target.group` (`voz`, etc.).
pub fn record_curated_task_in_group(
    repo: &Path,
    title: &str,
    summary: &str,
    status: TaskStatus,
    files: &[String],
    target: TaskTarget<'_>,
) -> Result<(TaskMeta, TaskEntry)> {
    let clean_title = validate_curated_task_title(title)?;
    let clean_summary = summary.trim();
    if clean_summary.is_empty() {
        bail!("task summary must not be empty");
    }
    let resolved_author = match target.author {
        Some(a) => a.to_owned(),
        None => resolve_author_fast(repo).or_else(|_| resolve_author(repo))?,
    };
    let merged_files = collect_task_files(repo, None, files);
    let meta = create_task_meta_in_group(
        repo,
        &resolved_author,
        &clean_title,
        target.group,
        target.task_id,
    )?;
    let entry = append_entry_as(
        repo,
        &meta.task_id,
        &resolved_author,
        status,
        clean_summary,
        &merged_files,
    )?;
    build_root_tasks_json(repo)?;
    mark_summary_recorded_with_files(repo, &meta.task_id, status, &entry.files_touched)?;
    clear_untracked_edits(repo);
    Ok((meta, entry))
}
