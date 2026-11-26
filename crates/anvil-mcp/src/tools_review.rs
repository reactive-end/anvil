//! MCP tool handlers for `review_log`, `review_packet`, and `review_approve`.

use anvil_core::review_gate;
use anvil_core::stats::{NewFinding, log_review};
use anvil_tasks::review;
use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::McpServer;

/// Handles `review_log`; arguments mirror `anvil review log`.
pub fn call_review_log(server: &McpServer, args: &Value) -> Result<Value> {
    let text = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let finding = NewFinding {
        description: text("description")
            .context("missing or empty required string argument `description`")?,
        category: text("category")
            .context("missing or empty required string argument `category`")?,
        severity: text("severity").unwrap_or_else(|| "medium".to_owned()),
        source: text("source").unwrap_or_else(|| "orchestrator".to_owned()),
        file: text("file"),
    };
    let logged = log_review(server.repo_root(), finding)?;
    Ok(json!({
        "recorded": true,
        "finding": logged.entry,
        "after_green": logged.entry.after_green,
        "new_category": logged.new_category,
        "known_categories": logged.known_categories,
    }))
}

/// Handles `review_packet`; mirrors `anvil review packet [task_id]`.
pub fn call_review_packet(server: &McpServer, args: &Value) -> Result<Value> {
    let task_id = args.get("task_id").and_then(Value::as_str);
    let packet = review::packet(server.repo_root(), task_id)?;
    Ok(json!({ "task_id": packet.task_id, "files": packet.files, "packet": packet.text }))
}

/// Handles `review_approve`; mirrors `anvil review approve <task_id> <summary>`.
pub fn call_review_approve(server: &McpServer, args: &Value) -> Result<Value> {
    let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::trim).unwrap_or_default();
    let task_id = text("task_id");
    if task_id.is_empty() {
        anyhow::bail!("missing or empty required string argument `task_id`");
    }
    let approval = review_gate::approve(server.repo_root(), task_id, text("summary"))?;
    Ok(json!({ "approved": true, "task_id": task_id, "summary": approval.summary }))
}
