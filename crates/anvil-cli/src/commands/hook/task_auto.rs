//! Hands-free automatic task tracking helpers for `anvil hook`.

use std::path::Path;

use anvil_adapters::contract::{HookOutcome, InboxMessage, MessageLevel, NormalizedHookInput};
use anvil_tasks::ledger::tasks_root;
use anvil_tasks::{TaskStatus, build_root_tasks_json, ensure_task_for_prompt, load_active_task};

/// Auto-creates or reuses an active task on Antigravity `PreInvocation` and Claude `UserPromptSubmit`.
pub fn handle_prompt_hook(root: &Path, input: &NormalizedHookInput) -> HookOutcome {
    let Some(prompt) = input.user_prompt.as_deref() else {
        return HookOutcome::allow();
    };
    let Ok(Some((state, true))) = ensure_task_for_prompt(root, prompt) else {
        return HookOutcome::allow();
    };
    let msg = format!(
        "Auto-registered task {}. When finished editing files, record a concise summary via: anvil tasks done {} \"<summary>\" (or update if still in progress).",
        state.task_id, state.task_id
    );
    HookOutcome::with_messages(vec![InboxMessage::new(
        "anvil-tasks",
        MessageLevel::Info,
        msg,
        None,
    )])
}

/// Ensures `tasks.json` is freshly rebuilt whenever `.harness/tasks/` exists.
pub fn refresh_tasks_json_if_present(root: &Path) {
    if tasks_root(root).is_dir() {
        let _ = build_root_tasks_json(root);
    }
}

/// Blocks `Stop` once if non-harness files changed and the active task lacks a recorded summary.
pub fn check_pending_task_summary(root: &Path, changed: &[String]) -> Option<HookOutcome> {
    let has_project_changes = changed
        .iter()
        .any(|p| !p.starts_with(".harness/") && p != "tasks.json");
    if !has_project_changes {
        return None;
    }
    let Ok(Some(state)) = load_active_task(root) else {
        return None;
    };
    if state.summary_recorded_at.is_some() || state.status == TaskStatus::Done {
        return None;
    }
    Some(HookOutcome::block_stop(format!(
        "Files were modified under task {}. Record a concise summary before finishing: anvil tasks done {} \"<1-2 sentence summary of changes>\"",
        state.task_id, state.task_id
    )))
}
