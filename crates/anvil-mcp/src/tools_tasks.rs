//! MCP tool handlers for LLM-curated task governance (`task_record`, `task_done`, `task_skip`, `task_list`).

use std::path::Path;

use anvil_tasks::{
    ProgressUpdate, TaskStatus, TaskTarget, activate_task_from_meta, begin_existing_task,
    build_resume_brief, clear_untracked_edits, collect_task_files, consolidate_repo_tasks,
    read_single_task, record_curated_task_in_group, record_task_progress, record_task_skip,
    resolve_author_fast, start_task_in_group, task_progress_rel_path, validate_curated_task_title,
};
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::McpServer;

/// Dispatches `task_*` MCP tools against the repository's `.harness/tasks/` ledger.
pub fn call_task_tool(server: &McpServer, name: &str, args: &Value) -> Result<Value> {
    let root = server.repo_root();
    match name {
        "task_start" => handle_task_start(root, args),
        "task_update" => handle_task_update(root, args),
        "task_record" => handle_task_record(root, args),
        "task_done" => handle_task_done(root, args),
        "task_skip" => handle_task_skip(root, args),
        "task_list" => handle_task_list(root, args),
        "task_get" => handle_task_get(root, args),
        "task_resume" => handle_task_resume(root, args),
        other => bail!("unknown task tool: {other}"),
    }
}

fn handle_task_start(root: &Path, args: &Value) -> Result<Value> {
    let author = resolve_author_fast(root).unwrap_or_else(|_| "mcp".to_owned());
    let task_id = opt_str(args, "task_id");
    let title = opt_str(args, "title");
    let group = opt_str(args, "group");
    let summary = opt_str(args, "summary");

    if let Some(id) = task_id
        && title.is_none()
    {
        let (task, entry) = begin_existing_task(root, id, &author, summary)?;
        return Ok(json!({
            "task_id": task.task_id,
            "title": task.request,
            "status": task.status.to_string(),
            "entry_id": entry.entry_id,
            "progress_file": task_progress_rel_path(&task.task_id),
        }));
    }

    let raw_title = title.ok_or_else(|| {
        anyhow::anyhow!("provide an existing `task_id` or a concise `title` to start a task")
    })?;
    let clean_title = validate_curated_task_title(raw_title)?;
    let meta = start_task_in_group(root, &author, &clean_title, group, task_id)?;
    let _ = anvil_tasks::build_root_tasks_json(root)?;
    activate_task_from_meta(root, &meta)?;
    if let Some(note) = summary {
        let _ = record_task_progress(
            root,
            &meta.task_id,
            &author,
            ProgressUpdate {
                status: TaskStatus::InProgress,
                summary: note,
                ..ProgressUpdate::default()
            },
        )?;
    }
    Ok(json!({
        "task_id": meta.task_id,
        "title": meta.request,
        "status": "in_progress",
        "progress_file": task_progress_rel_path(&meta.task_id),
    }))
}

fn handle_task_update(root: &Path, args: &Value) -> Result<Value> {
    let task_id = req_str(args, "task_id")?;
    let summary = req_str(args, "summary")?;
    let status = match opt_str(args, "status") {
        Some("blocked") => TaskStatus::Blocked,
        Some("done") => TaskStatus::Done,
        Some("not_started") => TaskStatus::NotStarted,
        Some("in_progress") | None => TaskStatus::InProgress,
        Some(other) => bail!("invalid task status `{other}`"),
    };
    let done_steps = opt_string_vec(args, "done_steps");
    let next_steps = opt_string_vec(args, "next_steps");
    let blocker = opt_str(args, "blocker");
    let evidence = opt_str(args, "evidence");
    let explicit_files = opt_string_vec(args, "files");
    let files = collect_task_files(root, Some(task_id), &explicit_files);
    let author = resolve_author_fast(root).unwrap_or_else(|_| "mcp".to_owned());

    let (task, entry) = record_task_progress(
        root,
        task_id,
        &author,
        ProgressUpdate {
            status,
            summary,
            done_steps: &done_steps,
            next_steps: &next_steps,
            blocker,
            evidence,
            files: &files,
        },
    )?;
    Ok(json!({
        "task_id": task.task_id,
        "status": task.status.to_string(),
        "entry_id": entry.entry_id,
        "progress_file": task_progress_rel_path(&task.task_id),
        "files_touched": task.files_touched,
        "done_steps": entry.done_steps,
        "next_steps": entry.next_steps,
    }))
}

fn handle_task_record(root: &Path, args: &Value) -> Result<Value> {
    let title = req_str(args, "title")?;
    let summary = req_str(args, "summary")?;
    let status = match args.get("status").and_then(Value::as_str) {
        Some("not_started") => TaskStatus::NotStarted,
        Some("in_progress") => TaskStatus::InProgress,
        Some("blocked") => TaskStatus::Blocked,
        Some("done") | None => TaskStatus::Done,
        Some(other) => bail!("invalid task status `{other}`"),
    };
    let group = opt_str(args, "group");
    let explicit_id = opt_str(args, "task_id");
    let explicit_files = opt_string_vec(args, "files");
    let (meta, entry) = record_curated_task_in_group(
        root,
        title,
        summary,
        status,
        &explicit_files,
        TaskTarget { author: None, group, task_id: explicit_id },
    )?;
    Ok(json!({
        "task_id": meta.task_id,
        "title": meta.request,
        "status": entry.status.to_string(),
        "entry_id": entry.entry_id,
        "progress_file": task_progress_rel_path(&meta.task_id),
        "files_touched": entry.files_touched,
    }))
}

fn handle_task_done(root: &Path, args: &Value) -> Result<Value> {
    let task_id = req_str(args, "task_id")?;
    let summary = req_str(args, "summary")?;
    let evidence = opt_str(args, "evidence");
    let explicit_files = opt_string_vec(args, "files");
    let files = collect_task_files(root, Some(task_id), &explicit_files);
    let author = resolve_author_fast(root).unwrap_or_else(|_| "mcp".to_owned());
    let (task, entry) = record_task_progress(
        root,
        task_id,
        &author,
        ProgressUpdate {
            status: TaskStatus::Done,
            summary,
            evidence,
            files: &files,
            ..ProgressUpdate::default()
        },
    )?;
    clear_untracked_edits(root);
    Ok(json!({
        "task_id": task.task_id,
        "status": "done",
        "entry_id": entry.entry_id,
        "progress_file": task_progress_rel_path(&task.task_id),
        "files_touched": task.files_touched,
    }))
}

fn handle_task_skip(root: &Path, args: &Value) -> Result<Value> {
    let reason = req_str(args, "reason")?;
    let state = record_task_skip(root, reason)?;
    Ok(json!({
        "skipped": state.skipped,
        "reason": state.skip_reason,
    }))
}

fn handle_task_list(root: &Path, args: &Value) -> Result<Value> {
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(15)
        .clamp(1, 100);
    let status_filter = match args.get("status").and_then(Value::as_str) {
        Some("not_started") => Some(TaskStatus::NotStarted),
        Some("in_progress") => Some(TaskStatus::InProgress),
        Some("blocked") => Some(TaskStatus::Blocked),
        Some("done") => Some(TaskStatus::Done),
        Some(other) => bail!("invalid task status filter `{other}`"),
        None => None,
    };
    let group_filter =
        args.get("group").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let consolidated = consolidate_repo_tasks(root)?;
    let slice: Vec<Value> = consolidated
        .tasks
        .into_iter()
        .filter(|t| status_filter.is_none_or(|s| t.status == s))
        .filter(|t| {
            group_filter.is_none_or(|g| {
                t.task_id.split_once('/').is_some_and(|(pfx, _)| pfx.eq_ignore_ascii_case(g))
                    || t.project.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(g))
            })
        })
        .rev()
        .take(limit)
        .map(|t| {
            json!({
                "task_id": t.task_id,
                "title": t.request,
                "status": t.status.to_string(),
                "project": t.project,
                "has_verify": t.verify.is_some(),
                "evidence": t.evidence,
            })
        })
        .collect();
    Ok(json!({ "tasks": slice }))
}

fn handle_task_get(root: &Path, args: &Value) -> Result<Value> {
    let task_id = req_str(args, "task_id")?;
    let task = read_single_task(root, task_id)?;
    Ok(serde_json::to_value(task)?)
}

fn handle_task_resume(root: &Path, args: &Value) -> Result<Value> {
    let task_id = opt_str(args, "task_id");
    let Some(brief) = build_resume_brief(root, task_id)? else {
        return Ok(json!({ "found": false, "brief": null }));
    };
    let progress_markdown = std::fs::read_to_string(root.join(&brief.progress_file)).ok();
    Ok(json!({
        "found": true,
        "brief": brief,
        "progress_markdown": progress_markdown,
    }))
}

fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    opt_str(args, key).ok_or_else(|| anyhow::anyhow!("missing required non-empty string `{key}`"))
}

fn opt_string_vec(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
