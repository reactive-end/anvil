//! Non-destructive generator and merger for `.claude/settings.json` and `.mcp.json`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::antigravity::config::{
    ensure_array_field, ensure_object_field, merge_mcp_servers_json,
    merge_umbrella_mcp_servers_json, read_json_object,
};
use crate::contract::HookEvent;
use crate::sync::write_if_changed;

const CLAUDE_EVENTS: &[HookEvent] = &[
    HookEvent::SessionStart,
    HookEvent::UserPromptSubmit,
    HookEvent::PreToolUse,
    HookEvent::PostToolUse,
    HookEvent::Stop,
];

/// Returns the path to `.claude/settings.json`.
pub fn settings_config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".claude").join("settings.json")
}

/// Returns the path to root `.mcp.json`.
pub fn mcp_config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".mcp.json")
}

/// Non-destructively merges `anvil hook` handlers into `.claude/settings.json`.
pub fn merge_settings_config(repo_root: &Path) -> Result<bool> {
    let path = settings_config_path(repo_root);
    let mut root = read_json_object(&path)?;

    let hooks_obj = ensure_object_field(&mut root, "hooks")?;
    for event in CLAUDE_EVENTS {
        let key = event.as_pascal_case();
        let cli_event = match event {
            HookEvent::UserPromptSubmit => "user-prompt-submit",
            _ => key,
        };
        let command = format!("anvil hook {cli_event} --adapter claude");
        let use_matcher = matches!(event, HookEvent::PreToolUse | HookEvent::PostToolUse);
        let list = ensure_array_field(hooks_obj, key)?;
        upsert_claude_hook_entry(list, &command, use_matcher);
    }

    let formatted = serde_json::to_string_pretty(&Value::Object(root))
        .context("failed to serialize .claude/settings.json")?;
    write_if_changed(&path, &format!("{formatted}\n"))
}

/// Non-destructively merges the `anvil mcp` server entry into root `.mcp.json`.
pub fn merge_mcp_config(repo_root: &Path) -> Result<bool> {
    merge_mcp_servers_json(&mcp_config_path(repo_root), ".mcp.json")
}

/// Non-destructively merges per-subrepo `anvil-<slug>` MCP entries into `.mcp.json` for Thin Umbrella roots.
pub fn merge_umbrella_mcp_config(root: &Path, subrepos: &[(String, PathBuf)]) -> Result<bool> {
    merge_umbrella_mcp_servers_json(&mcp_config_path(root), ".mcp.json", subrepos)
}

fn upsert_claude_hook_entry(entries: &mut Vec<Value>, command: &str, use_matcher: bool) {
    let hook_item = json!({
        "type": "command",
        "command": command
    });
    if let Some(existing) = entries.iter_mut().find(|e| is_anvil_claude_entry(e))
        && let Some(arr) = existing.get_mut("hooks").and_then(Value::as_array_mut)
        && let Some(item) = arr.iter_mut().find(|h| is_anvil_hook_item(h))
    {
        *item = hook_item;
        return;
    }
    let new_entry = if use_matcher {
        json!({ "matcher": "*", "hooks": [hook_item] })
    } else {
        json!({ "hooks": [hook_item] })
    };
    entries.push(new_entry);
}

fn is_anvil_claude_entry(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|arr| arr.iter().any(is_anvil_hook_item))
}

fn is_anvil_hook_item(item: &Value) -> bool {
    item.get("command").and_then(Value::as_str).is_some_and(|cmd| cmd.starts_with("anvil hook "))
}
