//! `anvil tasks`: conflict-free team task ledger and `tasks.json` builder.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Result, bail};
use anvil_core::git::{self, DiffScope};
use anvil_tasks::{
    TaskStatus, activate_task_from_meta, append_entry, append_entry_as, build_root_tasks_json,
    list_tasks, load_active_task, mark_summary_recorded_with_files, resolve_author, resolve_merge,
    start_task, start_task_as, write_task_plan,
};

use crate::cli::subcmds::TasksSubcmd;

/// Executes `anvil tasks <subcommand>`.
pub fn execute(root: &Path, sub: TasksSubcmd) -> Result<()> {
    match sub {
        TasksSubcmd::Start { request, author } => cmd_start(root, &request.join(" "), author.as_deref()),
        TasksSubcmd::Update { task_id, summary, status, files, author } => {
            cmd_append(root, &task_id, &summary, parse_status(&status)?, &files, author.as_deref())
        }
        TasksSubcmd::Done { task_id, summary, files, author } => {
            cmd_append(root, &task_id, &summary, TaskStatus::Done, &files, author.as_deref())
        }
        TasksSubcmd::Plan { task_id, markdown_or_file, author } => {
            cmd_plan(root, &task_id, &markdown_or_file, author)
        }
        TasksSubcmd::Build => {
            let consolidated = build_root_tasks_json(root)?;
            println!("built tasks.json: {} task(s)", consolidated.task_count);
            Ok(())
        }
        TasksSubcmd::List => cmd_list(root),
        TasksSubcmd::MergeDriver { base, current, other } => {
            let consolidated = resolve_merge(&base, &current, &other, root)?;
            println!("merged tasks.json cleanly: {} task(s)", consolidated.task_count);
            Ok(())
        }
    }
}

fn cmd_start(root: &Path, request: &str, author: Option<&str>) -> Result<()> {
    let meta = match author {
        Some(a) => start_task_as(root, a, request)?,
        None => start_task(root, request)?,
    };
    let consolidated = build_root_tasks_json(root)?;
    activate_task_from_meta(root, &meta)?;
    println!(
        "started task {} by {} (tasks.json total={})",
        meta.task_id, meta.created_by, consolidated.task_count
    );
    Ok(())
}

fn cmd_append(
    root: &Path,
    task_id: &str,
    summary: &str,
    status: TaskStatus,
    files: &[String],
    author: Option<&str>,
) -> Result<()> {
    let resolved_files = resolve_touched_files(root, task_id, files);
    let entry = match author {
        Some(a) => append_entry_as(root, task_id, a, status, summary, &resolved_files)?,
        None => append_entry(root, task_id, status, summary, &resolved_files)?,
    };
    let consolidated = build_root_tasks_json(root)?;
    mark_summary_recorded_with_files(root, task_id, status, &entry.files_touched)?;
    println!(
        "updated {} [{}] entry={} files={} (tasks.json total={})",
        task_id,
        entry.status,
        entry.entry_id,
        entry.files_touched.len(),
        consolidated.task_count
    );
    if status == TaskStatus::Done && !entry.files_touched.is_empty() && looks_like_learning(summary)
    {
        println!(
            "memory-hint: consider saving durable root-cause/decision via `anvil mem save` (or MCP `mem_save`) with related_files."
        );
    }
    Ok(())
}

fn looks_like_learning(summary: &str) -> bool {
    let lower = summary.to_ascii_lowercase();
    ["fix", "bug", "error", "gotcha", "workaround", "auth", "oauth", "2fa", "corr", "fallback"]
        .iter()
        .any(|kw| lower.contains(kw))
}

/// Uses explicit `--files` when provided; otherwise auto-collects files from `active-task.json`
/// and `git status` working-tree changes (excluding `.harness/` metadata and `tasks.json`).
fn resolve_touched_files(root: &Path, task_id: &str, explicit: &[String]) -> Vec<String> {
    if !explicit.is_empty() {
        return explicit.to_vec();
    }
    let mut set = BTreeSet::new();
    if let Ok(Some(active)) = load_active_task(root)
        && active.task_id == task_id
    {
        for f in active.touched_files {
            if is_project_file(&f) {
                set.insert(f);
            }
        }
    }
    if let Ok(changed) = git::changed_files(root, DiffScope::WorkingTree) {
        for f in changed {
            let norm = f.replace('\\', "/");
            if is_project_file(&norm) {
                set.insert(norm);
            }
        }
    }
    set.into_iter().collect()
}

fn is_project_file(path: &str) -> bool {
    let trimmed = path.trim();
    !trimmed.is_empty() && !trimmed.starts_with(".harness/") && trimmed != "tasks.json"
}

fn cmd_plan(root: &Path, task_id: &str, markdown_or_file: &str, author: Option<String>) -> Result<()> {
    let resolved_author = match author {
        Some(a) => a,
        None => resolve_author(root)?,
    };
    let md = if Path::new(markdown_or_file).is_file() {
        fs::read_to_string(markdown_or_file)?
    } else {
        markdown_or_file.to_owned()
    };
    let path = write_task_plan(root, task_id, &resolved_author, &md)?;
    println!("wrote plan: {}", path.display());
    Ok(())
}

fn cmd_list(root: &Path) -> Result<()> {
    let metas = list_tasks(root)?;
    if metas.is_empty() {
        println!("no tasks recorded in .harness/tasks/");
        return Ok(());
    }
    for meta in metas {
        println!("{} [{}] {}", meta.task_id, meta.created_by, meta.request);
    }
    Ok(())
}

fn parse_status(raw: &str) -> Result<TaskStatus> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "in_progress" | "in-progress" | "progress" => Ok(TaskStatus::InProgress),
        "blocked" => Ok(TaskStatus::Blocked),
        "done" | "completed" => Ok(TaskStatus::Done),
        other => bail!("invalid task status `{other}` (expected in_progress|blocked|done)"),
    }
}
