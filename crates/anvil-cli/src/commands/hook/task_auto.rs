//! Hands-free automatic task tracking helpers for `anvil hook`.

use std::fs;
use std::path::Path;
use std::time::SystemTime;

use anvil_adapters::contract::{HookOutcome, NormalizedHookInput};
use anvil_tasks::ledger::tasks_root;
use anvil_tasks::{
    TaskStatus, build_root_tasks_json, load_active_task, materialize_task_on_edit,
    pending_prompt_path, record_pending_prompt, record_touched_file,
};

/// Records a pending user prompt in `.harness/.state/pending-prompt.json` (gitignored) without
/// creating a task yet; tasks materialize lazily on the first real file edit in `PostToolUse`.
pub fn handle_prompt_hook(root: &Path, input: &NormalizedHookInput) -> HookOutcome {
    if let Some(prompt) = input.user_prompt.as_deref() {
        let _ = record_pending_prompt(root, prompt);
    }
    HookOutcome::allow()
}

/// Rebuilds `tasks.json` ONLY when `.harness/tasks/` has newer metadata or entries than `tasks.json`.
pub fn refresh_tasks_json_if_changed(root: &Path) {
    let t_root = tasks_root(root);
    if !t_root.is_dir() {
        return;
    }
    if needs_tasks_rebuild(root, &t_root) {
        let _ = build_root_tasks_json(root);
    }
}

fn needs_tasks_rebuild(root: &Path, t_root: &Path) -> bool {
    let tasks_json = root.join("tasks.json");
    let Ok(json_mtime) = fs::metadata(&tasks_json).and_then(|m| m.modified()) else {
        return true;
    };
    if is_newer(t_root, json_mtime) {
        return true;
    }
    let Ok(entries) = fs::read_dir(t_root) else {
        return true;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        if is_newer(&dir.join("meta.json"), json_mtime)
            || is_newer(&dir.join("entries"), json_mtime)
        {
            return true;
        }
    }
    false
}

fn is_newer(path: &Path, baseline: SystemTime) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|mtime| mtime > baseline)
}

/// Blocks `Stop` once if non-harness files changed and the active task lacks a recorded summary.
/// If files were modified while a pending prompt exists without an active task (e.g. edits via
/// shell commands or external tools), materializes the task on the spot so no code edit goes untracked.
pub fn check_pending_task_summary(root: &Path, changed: &[String]) -> Option<HookOutcome> {
    let project_changes: Vec<&String> = changed
        .iter()
        .filter(|p| !p.starts_with(".harness/") && *p != "tasks.json")
        .collect();
    if project_changes.is_empty() {
        return None;
    }

    let state = match load_active_task(root).ok().flatten() {
        Some(existing) if existing.status == TaskStatus::InProgress => existing,
        Some(existing) if existing.summary_recorded_at.is_some() || existing.status == TaskStatus::Done => {
            if pending_prompt_path(root).is_file() {
                materialize_from_stop(root, &project_changes)?
            } else {
                return None;
            }
        }
        Some(existing) => existing,
        None => {
            if pending_prompt_path(root).is_file() {
                materialize_from_stop(root, &project_changes)?
            } else {
                return None;
            }
        }
    };

    if state.summary_recorded_at.is_some() || state.status == TaskStatus::Done {
        return None;
    }
    Some(HookOutcome::block_stop(format!(
        "Files were modified under task {}. Record a concise summary before finishing: anvil tasks done {} \"<1-2 sentence summary of changes>\"",
        state.task_id, state.task_id
    )))
}

fn materialize_from_stop(
    root: &Path,
    project_changes: &[&String],
) -> Option<anvil_tasks::ActiveTaskState> {
    let first = project_changes.first()?;
    let state = materialize_task_on_edit(root, first).ok().flatten()?;
    for extra in project_changes.iter().skip(1) {
        let _ = record_touched_file(root, extra);
    }
    Some(state)
}
