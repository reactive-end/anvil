//! MCP tool handlers for LLM-curated task governance (`task_record`, `task_done`, `task_skip`, `task_list`).

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Result, bail};
use anvil_core::git::{self, DiffScope};
use anvil_tasks::{
    TaskStatus, append_entry_as, build_root_tasks_json, clear_untracked_edits, list_tasks,
    load_active_task, load_untracked_edits, mark_summary_recorded_with_files, record_curated_task,
    record_task_skip, resolve_author_fast,
};
use serde_json::{Value, json};

use crate::McpServer;

/// Dispatches `task_*` MCP tools against the repository's `.harness/tasks/` ledger.
pub fn call_task_tool(server: &McpServer, name: &str, args: &Value) -> Result<Value> {
    let root = server.repo_root();
    match name {
        "task_record" => handle_task_record(root, args),
        "task_done" => handle_task_done(root, args),
        "task_skip" => handle_task_skip(root, args),
        "task_list" => handle_task_list(root, args),
        other => bail!("unknown task tool: {other}"),
    }
}

fn handle_task_record(root: &Path, args: &Value) -> Result<Value> {
    let title = req_str(args, "title")?;
    let summary = req_str(args, "summary")?;
    let status = match args.get("status").and_then(Value::as_str) {
        Some("in_progress") => TaskStatus::InProgress,
        Some("blocked") => TaskStatus::Blocked,
        _ => TaskStatus::Done,
    };
    let explicit_files = opt_string_vec(args, "files");
    let (meta, entry) = record_curated_task(root, title, summary, status, &explicit_files, None)?;
    Ok(json!({
        "task_id": meta.task_id,
        "title": meta.request,
        "status": entry.status.to_string(),
        "entry_id": entry.entry_id,
        "files_touched": entry.files_touched,
    }))
}

fn handle_task_done(root: &Path, args: &Value) -> Result<Value> {
    let task_id = req_str(args, "task_id")?;
    let summary = req_str(args, "summary")?;
    let explicit_files = opt_string_vec(args, "files");
    let files = collect_task_files(root, task_id, &explicit_files);
    let author = resolve_author_fast(root).unwrap_or_else(|_| "mcp".to_owned());
    let entry = append_entry_as(root, task_id, &author, TaskStatus::Done, summary, &files)?;
    let consolidated = build_root_tasks_json(root)?;
    mark_summary_recorded_with_files(root, task_id, TaskStatus::Done, &entry.files_touched)?;
    clear_untracked_edits(root);
    Ok(json!({
        "task_id": task_id,
        "status": "done",
        "entry_id": entry.entry_id,
        "files_touched": entry.files_touched,
        "total_tasks": consolidated.task_count,
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
        .unwrap_or(10)
        .clamp(1, 100);
    let metas = list_tasks(root)?;
    let slice: Vec<Value> = metas
        .into_iter()
        .rev()
        .take(limit)
        .map(|m| {
            json!({
                "task_id": m.task_id,
                "title": m.request,
                "created_by": m.created_by,
                "created_at": m.created_at,
            })
        })
        .collect();
    Ok(json!({ "tasks": slice }))
}

fn collect_task_files(root: &Path, task_id: &str, explicit: &[String]) -> Vec<String> {
    if !explicit.is_empty() {
        return explicit.to_vec();
    }
    let mut set: BTreeSet<String> = load_untracked_edits(root).into_iter().collect();
    if let Ok(Some(active)) = load_active_task(root)
        && active.task_id == task_id
    {
        set.extend(active.touched_files);
    }
    if let Ok(changed) = git::changed_files(root, DiffScope::WorkingTree) {
        for f in changed {
            let norm = f.replace('\\', "/");
            if !norm.is_empty() && !norm.starts_with(".harness/") && norm != "tasks.json" {
                set.insert(norm);
            }
        }
    }
    set.into_iter().collect()
}

fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing required non-empty string `{key}`"))
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
