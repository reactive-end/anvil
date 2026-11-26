//! MCP tool handler for `review_log`: record a problem found in review of work that passed `anvil check`.

use anyhow::{Context, Result};
use anvil_core::stats::{NewFinding, log_review};
use serde_json::{Value, json};

use crate::McpServer;

/// Handles `review_log`; arguments mirror `anvil review log`.
pub fn call_review_log(server: &McpServer, args: &Value) -> Result<Value> {
    let text = |key: &str| {
        args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
    };
    let finding = NewFinding {
        description: text("description").context("missing or empty required string argument `description`")?,
        category: text("category").context("missing or empty required string argument `category`")?,
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
