//! Per-task `progress.md` rendering, structured progress logging, and crash-recovery briefs.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use anvil_core::fs_util::atomic_write;
use anvil_core::git::{self, DiffScope};
use serde::{Deserialize, Serialize};

use crate::active::{load_active_task, update_active_task_progress};
use crate::builder::{build_root_tasks_json, consolidate_repo_tasks, read_single_task};
use crate::ledger::{append_entry_with_details, task_dir, validate_task_id_path};
use crate::model::{ConsolidatedTask, TaskEntry, TaskStatus};

/// Structured parameters for recording a task progress milestone, transition, or completion.
#[derive(Debug, Clone, Copy)]
pub struct ProgressUpdate<'a> {
    pub status: TaskStatus,
    pub summary: &'a str,
    pub done_steps: &'a [String],
    pub next_steps: &'a [String],
    pub blocker: Option<&'a str>,
    pub evidence: Option<&'a str>,
    pub files: &'a [String],
}

impl Default for ProgressUpdate<'_> {
    fn default() -> Self {
        Self {
            status: TaskStatus::InProgress,
            summary: "",
            done_steps: &[],
            next_steps: &[],
            blocker: None,
            evidence: None,
            files: &[],
        }
    }
}

/// Compact crash-recovery context returned by `task_resume` / `anvil tasks resume` / `SessionStart`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskResumeBrief {
    pub task_id: String,
    pub title: String,
    pub status: TaskStatus,
    pub progress_file: String,
    pub summary: String,
    pub done_steps: Vec<String>,
    pub next_steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<String>,
    pub touched_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_command_passed: Option<bool>,
    pub dirty_files: Vec<String>,
}

/// Returns the repo-relative path `.harness/tasks/<task_id>/progress.md`.
pub fn task_progress_rel_path(task_id: &str) -> String {
    format!(".harness/tasks/{}/progress.md", task_id.trim().replace('\\', "/"))
}

/// Transitions an existing queued/paused task to `in_progress`, writes `progress.md`,
/// updates `tasks.json`, and activates it in `.harness/.state/active-task.json`.
pub fn begin_existing_task(
    repo: &Path,
    task_id: &str,
    author: &str,
    summary: Option<&str>,
) -> Result<(ConsolidatedTask, TaskEntry)> {
    let valid_id = validate_task_id_path(task_id)?;
    let meta_path = task_dir(repo, &valid_id).join("meta.json");
    if !meta_path.is_file() {
        bail!("task `{valid_id}` not found at {}", meta_path.display());
    }
    let note = summary
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| format!("Started task {valid_id}"), str::to_owned);
    record_task_progress(
        repo,
        &valid_id,
        author,
        ProgressUpdate {
            status: TaskStatus::InProgress,
            summary: &note,
            ..ProgressUpdate::default()
        },
    )
}

/// Appends an immutable entry, renders `.harness/tasks/<id>/progress.md`, rebuilds `tasks.json`,
/// and updates `.harness/.state/active-task.json`.
pub fn record_task_progress(
    repo: &Path,
    task_id: &str,
    author: &str,
    update: ProgressUpdate<'_>,
) -> Result<(ConsolidatedTask, TaskEntry)> {
    let clean_summary = update.summary.trim();
    if clean_summary.is_empty() {
        bail!("progress summary must not be empty");
    }
    let valid_id = validate_task_id_path(task_id)?;
    let normalized = ProgressUpdate {
        summary: clean_summary,
        ..update
    };
    let entry = append_entry_with_details(repo, &valid_id, author, &normalized)?;
    let _ = build_root_tasks_json(repo)?;
    let task = read_single_task(repo, &valid_id)?;
    update_active_task_progress(
        repo,
        &valid_id,
        normalized.status,
        clean_summary,
        normalized.next_steps,
        &entry.files_touched,
    )?;
    Ok((task, entry))
}

/// Regenerates `.harness/tasks/<task_id>/progress.md` from `meta.json` and all `entries/*.json`.
pub fn write_task_progress_md(repo: &Path, task_id: &str) -> Result<PathBuf> {
    let valid_id = validate_task_id_path(task_id)?;
    let task = read_single_task(repo, &valid_id)?;
    ensure_tasks_gitignore(repo)?;
    let path = task_dir(repo, &valid_id).join("progress.md");
    let rendered = render_task_progress_md(&task);
    atomic_write(&path, rendered.as_bytes())?;
    Ok(path)
}

fn ensure_tasks_gitignore(repo: &Path) -> Result<()> {
    let path = crate::ledger::tasks_root(repo).join(".gitignore");
    let existing = if path.exists() {
        std::fs::read_to_string(&path).unwrap_or_default()
    } else {
        String::new()
    };
    if existing.lines().any(|line| line.trim() == "progress.md") {
        return Ok(());
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(
        "# Derived local progress views rebuilt from meta.json + entries/*.json\nprogress.md\n",
    );
    atomic_write(&path, updated.as_bytes())
}

/// Renders deterministic markdown summarizing task metadata, latest checkpoint, and timeline.
pub fn render_task_progress_md(task: &ConsolidatedTask) -> String {
    let mut out = String::new();
    append_header_section(&mut out, task);
    append_latest_section(&mut out, task);
    append_timeline_section(&mut out, &task.entries);
    out
}

/// Builds a compact recovery brief for `explicit_id`, the active `in_progress` task,
/// or the most recently updated `in_progress` task in the ledger.
pub fn build_resume_brief(
    repo: &Path,
    explicit_id: Option<&str>,
) -> Result<Option<TaskResumeBrief>> {
    let active = load_active_task(repo).ok().flatten();
    let Some(target_id) = resolve_resume_target_id(repo, explicit_id, active.as_ref())? else {
        return Ok(None);
    };
    let task = read_single_task(repo, &target_id)?;
    let _ = write_task_progress_md(repo, &target_id);

    let same_active = active.as_ref().filter(|a| a.task_id == task.task_id);
    let mut files: BTreeSet<String> = task.files_touched.iter().cloned().collect();
    if let Some(st) = same_active {
        files.extend(st.touched_files.iter().cloned());
    }
    let latest_steps = latest_step_fields(&task.entries);
    let next_steps = if task.status == TaskStatus::Done {
        Vec::new()
    } else if !latest_steps.1.is_empty() {
        latest_steps.1
    } else {
        same_active.map_or_else(Vec::new, |st| st.next_steps.clone())
    };
    let dirty_files = collect_dirty_project_files(repo);

    Ok(Some(TaskResumeBrief {
        progress_file: task_progress_rel_path(&task.task_id),
        task_id: task.task_id,
        title: task.request,
        status: task.status,
        summary: task.summary,
        done_steps: latest_steps.0,
        next_steps,
        blocker: latest_steps.2,
        evidence: task.evidence,
        verify: task.verify,
        touched_files: files.into_iter().collect(),
        last_command: same_active.and_then(|st| st.last_command.clone()),
        last_command_passed: same_active.and_then(|st| st.last_command_passed),
        dirty_files,
    }))
}

fn resolve_resume_target_id(
    repo: &Path,
    explicit_id: Option<&str>,
    active: Option<&crate::active::ActiveTaskState>,
) -> Result<Option<String>> {
    if let Some(raw) = explicit_id
        && !raw.trim().is_empty()
    {
        return Ok(Some(validate_task_id_path(raw)?));
    }
    if let Some(st) = active
        && !st.skipped
        && st.status == TaskStatus::InProgress
        && task_dir(repo, &st.task_id).join("meta.json").is_file()
    {
        return Ok(Some(st.task_id.clone()));
    }
    let consolidated = consolidate_repo_tasks(repo)?;
    let in_progress = consolidated
        .tasks
        .into_iter()
        .filter(|t| t.status == TaskStatus::InProgress)
        .max_by(|a, b| a.updated_at.cmp(&b.updated_at));
    Ok(in_progress.map(|t| t.task_id))
}

fn latest_step_fields(entries: &[TaskEntry]) -> (Vec<String>, Vec<String>, Option<String>) {
    let mut done_acc = Vec::new();
    for entry in entries {
        for step in &entry.done_steps {
            if !done_acc.iter().any(|s| s == step) {
                done_acc.push(step.clone());
            }
        }
    }
    let next_steps = match entries.last() {
        Some(last) if last.status != TaskStatus::Done => last.next_steps.clone(),
        _ => Vec::new(),
    };
    let blocker = entries.last().and_then(|e| e.blocker.clone());
    (done_acc, next_steps, blocker)
}

fn collect_dirty_project_files(repo: &Path) -> Vec<String> {
    let Ok(changed) = git::changed_files(repo, DiffScope::WorkingTree) else {
        return Vec::new();
    };
    let mut out = BTreeSet::new();
    for f in changed {
        let norm = f.replace('\\', "/");
        if !norm.is_empty() && !norm.starts_with(".harness/") && norm != "tasks.json" {
            out.insert(norm);
        }
    }
    out.into_iter().collect()
}

fn append_header_section(out: &mut String, task: &ConsolidatedTask) {
    let _ = writeln!(out, "# {} — {}", task.task_id, task.request);
    let _ = writeln!(out);
    let _ = writeln!(out, "- **Status**: `{}`", task.status);
    let _ = writeln!(out, "- **Updated**: {}", task.updated_at.to_rfc3339());
    let _ = writeln!(out, "- **Authors**: {}", task.contributors.join(", "));
    if let Some(proj) = task.project.as_deref() {
        let _ = writeln!(out, "- **Project/Group**: `{proj}`");
    }
    if let Some(verify) = task.verify.as_deref() {
        let _ = writeln!(out, "- **Verify**: `{verify}`");
    }
    if let Some(behavior) = task.behavior.as_deref() {
        let _ = writeln!(out, "- **Behavior**: {behavior}");
    }
    if !task.files_touched.is_empty() {
        let _ = writeln!(out, "- **Files Touched**: {}", task.files_touched.join(", "));
    }
}

fn append_latest_section(out: &mut String, task: &ConsolidatedTask) {
    let (done_steps, next_steps, blocker) = latest_step_fields(&task.entries);
    let _ = writeln!(out, "\n## Current State\n");
    let _ = writeln!(out, "- **Summary**: {}", task.summary);
    if let Some(ev) = task.evidence.as_deref() {
        let _ = writeln!(out, "- **Evidence**: {ev}");
    }
    if let Some(blk) = blocker.as_deref() {
        let _ = writeln!(out, "- **Blocker**: {blk}");
    }
    if !done_steps.is_empty() {
        let _ = writeln!(out, "\n### Done Steps");
        for step in done_steps {
            let _ = writeln!(out, "- [x] {step}");
        }
    }
    if !next_steps.is_empty() && task.status != TaskStatus::Done {
        let _ = writeln!(out, "\n### Next Steps");
        for step in next_steps {
            let _ = writeln!(out, "- [ ] {step}");
        }
    }
}

fn append_timeline_section(out: &mut String, entries: &[TaskEntry]) {
    if entries.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n## Progress Timeline\n");
    for entry in entries {
        let _ = writeln!(
            out,
            "- **{}** [`{}`] `{}` — {}",
            entry.timestamp.to_rfc3339(),
            entry.status,
            entry.author,
            entry.summary
        );
        if !entry.done_steps.is_empty() {
            let _ = writeln!(out, "  - Done: {}", entry.done_steps.join("; "));
        }
        if !entry.next_steps.is_empty() {
            let _ = writeln!(out, "  - Next: {}", entry.next_steps.join("; "));
        }
        if let Some(blk) = entry.blocker.as_deref() {
            let _ = writeln!(out, "  - Blocker: {blk}");
        }
        if !entry.files_touched.is_empty() {
            let _ = writeln!(out, "  - Files: {}", entry.files_touched.join(", "));
        }
    }
}
