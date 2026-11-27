//! LLM-curated task governance helpers and strict anti-noise `Stop` gate for `anvil hook`.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

use anvil_adapters::contract::{
    HookEvent, HookOutcome, InboxMessage, MessageLevel, NormalizedHookInput,
};
use anvil_core::locale::Lang;
use anvil_tasks::ledger::tasks_root;
use anvil_tasks::{
    TaskStatus, build_root_tasks_json, clear_skipped_active_task, list_tasks, load_active_task,
    record_pending_prompt,
};

const TRIVIAL_SINGLE_FILE_MAX_LINES: usize = 12;
const TURN_EDITS_FLAG: &str = ".harness/.state/turn-edits.flag";

/// Records a non-conversational user prompt snapshot in `.harness/.state/pending-prompt.json`
/// without creating any task on disk, resetting turn edit state ONLY on a fresh user prompt.
pub fn handle_prompt_hook(root: &Path, input: &NormalizedHookInput) -> HookOutcome {
    if !root.join(".harness").is_dir() {
        return HookOutcome::allow();
    }
    if input.event == HookEvent::UserPromptSubmit || input.user_prompt.is_some() {
        clear_turn_edit(root);
        clear_skipped_active_task(root);
    }
    if let Some(prompt) = input.user_prompt.as_deref() {
        let _ = record_pending_prompt(root, prompt);
    }
    HookOutcome::allow()
}

/// Marks that the current agent turn performed at least one mutating file tool call.
pub fn mark_turn_edit(root: &Path) {
    if !root.join(".harness").is_dir() {
        return;
    }
    let flag = root.join(TURN_EDITS_FLAG);
    if let Some(parent) = flag.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(flag, b"1");
}

fn clear_turn_edit(root: &Path) {
    let _ = fs::remove_file(root.join(TURN_EDITS_FLAG));
}

/// Finalizes a passing `Stop` check so subsequent read-only turns avoid redundant `inbox.json` noise.
pub fn mark_stop_completed(root: &Path) {
    if !root.join(".harness").is_dir() {
        return;
    }
    clear_turn_edit(root);
    clear_skipped_active_task(root);
    let stamp = root.join(".harness/.state/last-stop.stamp");
    if let Some(parent) = stamp.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(stamp, b"ok");
}

/// Returns `true` when the turn is read-only (no `PostToolUse` edit flag, no untracked edits,
/// and no dirty file newer than the previous completed `Stop` stamp).
pub fn should_skip_readonly_stop(root: &Path, changed: &[String]) -> bool {
    if root.join(TURN_EDITS_FLAG).is_file() {
        return false;
    }
    if !anvil_tasks::load_untracked_edits(root).is_empty() {
        return false;
    }
    let Ok(stamp_mtime) =
        fs::metadata(root.join(".harness/.state/last-stop.stamp")).and_then(|m| m.modified())
    else {
        return false;
    };
    !changed.iter().any(|rel| is_newer(&root.join(rel), stamp_mtime))
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
        if dir.join("meta.json").is_file() {
            if is_newer(&dir.join("meta.json"), json_mtime)
                || is_newer(&dir.join("entries"), json_mtime)
            {
                return true;
            }
            continue;
        }
        if is_newer(&dir, json_mtime) {
            return true;
        }
        if let Ok(sub_entries) = fs::read_dir(&dir) {
            for sub in sub_entries.flatten() {
                let sdir = sub.path();
                if sdir.is_dir()
                    && (is_newer(&sdir.join("meta.json"), json_mtime)
                        || is_newer(&sdir.join("entries"), json_mtime))
                {
                    return true;
                }
            }
        }
    }
    false
}

fn is_newer(path: &Path, baseline: SystemTime) -> bool {
    fs::metadata(path).and_then(|m| m.modified()).is_ok_and(|mtime| mtime > baseline)
}

/// Enforces LLM-curated task decisions at `Stop` with a strict anti-noise bias:
/// - Read-only turns, pure docs edits, and `<= 12`-line single-file micro-tweaks pass automatically (`0` tasks).
/// - Never auto-creates tasks from raw user chat messages.
/// - On substantive diffs, asks the LLM once to record a curated task, reuse the latest task, or call `task_skip`.
/// - While an executor of the session is still working, the in-progress task is not closed yet.
pub fn check_pending_task_summary(
    root: &Path,
    changed: &[String],
    lang: Lang,
    executors_running: bool,
) -> Option<HookOutcome> {
    let project_changes: Vec<&String> =
        changed.iter().filter(|p| !p.starts_with(".harness/") && *p != "tasks.json").collect();
    if project_changes.is_empty() {
        return None;
    }

    let active = load_active_task(root).ok().flatten();
    if let Some(ref st) = active {
        if st.skipped {
            return None;
        }
        if st.status == TaskStatus::InProgress && st.summary_recorded_at.is_none() {
            if executors_running {
                return None;
            }
            return Some(HookOutcome::block_stop(super::texts::task_in_progress(
                lang,
                &st.task_id,
            )));
        }
    }

    let untracked = anvil_tasks::load_untracked_edits(root);
    if untracked.is_empty() && active.as_ref().is_some_and(|st| st.summary_recorded_at.is_some()) {
        return None;
    }

    let baseline = active
        .as_ref()
        .and_then(|st| {
            let progress_ts =
                (st.status == TaskStatus::Blocked).then_some(st.last_progress_at).flatten();
            st.summary_recorded_at.or(progress_ts).or(Some(st.started_at))
        })
        .map(SystemTime::from);
    let fresh_changes: Vec<&String> = match baseline {
        Some(base) => project_changes
            .into_iter()
            .filter(|rel| {
                let abs = root.join(rel.as_str());
                !abs.exists() || is_newer(&abs, base)
            })
            .collect(),
        None => project_changes,
    };
    if fresh_changes.is_empty() || is_trivial_edit_scope(root, &fresh_changes) {
        return None;
    }

    Some(HookOutcome::block_stop(build_curation_prompt(root, fresh_changes.len(), lang)))
}

/// A one-line note, instead of the demand to close the task, while an executor still works on it.
pub fn executor_running_advisory(root: &Path) -> Option<InboxMessage> {
    let active = load_active_task(root).ok().flatten()?;
    (active.status == TaskStatus::InProgress && active.summary_recorded_at.is_none()).then(|| {
        InboxMessage::new(
            "anvil-tasks",
            MessageLevel::Info,
            format!(
                "Task {} stays in progress while an executor is running; review it and call `task_done` once the executor finishes.",
                active.task_id
            ),
            None,
        )
    })
}

fn is_trivial_edit_scope(root: &Path, fresh_changes: &[&String]) -> bool {
    if fresh_changes.iter().all(|rel| is_doc_only_path(rel)) {
        return true;
    }
    if fresh_changes.len() == 1 {
        let changed_lines = count_changed_lines(root, fresh_changes[0]);
        return changed_lines <= TRIVIAL_SINGLE_FILE_MAX_LINES;
    }
    false
}

fn is_doc_only_path(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".mdx") || lower.ends_with(".txt")
}

fn count_changed_lines(root: &Path, rel: &str) -> usize {
    if let Ok(out) = Command::new("git")
        .args(["diff", "--unified=0", "HEAD", "--", rel])
        .current_dir(root)
        .output()
        && out.status.success()
    {
        let diff_text = String::from_utf8_lossy(&out.stdout);
        let diff_lines = diff_text
            .lines()
            .filter(|l| {
                (l.starts_with('+') && !l.starts_with("+++"))
                    || (l.starts_with('-') && !l.starts_with("---"))
            })
            .count();
        if diff_lines > 0 {
            return diff_lines;
        }
    }
    fs::read_to_string(root.join(rel))
        .map(|s| s.lines().count())
        .unwrap_or(TRIVIAL_SINGLE_FILE_MAX_LINES + 1)
}

fn build_curation_prompt(root: &Path, file_count: usize, lang: Lang) -> String {
    let last = list_tasks(root).ok().and_then(|mut v| v.pop());
    let last = last.as_ref().map(|task| (task.task_id.as_str(), task.request.as_str()));
    super::texts::curation_prompt(lang, file_count, last)
}
