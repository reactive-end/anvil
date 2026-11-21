//! Non-destructive generator and merger for `.claude/settings.json` and `.mcp.json`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::antigravity::config::{ensure_array_field, ensure_object_field, read_json_object};
use crate::atomic_fs;
use crate::contract::HookEvent;

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
    atomic_fs::write_if_changed(&path, &format!("{formatted}\n"))
}

/// Non-destructively merges the `anvil mcp` server entry into root `.mcp.json`.
pub fn merge_mcp_config(repo_root: &Path) -> Result<bool> {
    let path = mcp_config_path(repo_root);
    let mut root = read_json_object(&path)?;

    let servers = ensure_object_field(&mut root, "mcpServers")?;
    servers.insert(
        "anvil".to_owned(),
        json!({
            "command": "anvil",
            "args": ["mcp"]
        }),
    );

    let formatted = serde_json::to_string_pretty(&Value::Object(root))
        .context("failed to serialize .mcp.json")?;
    atomic_fs::write_if_changed(&path, &format!("{formatted}\n"))
}

fn upsert_claude_hook_entry(entries: &mut Vec<Value>, command: &str, use_matcher: bool) {
    let hook_item = json!({
        "type": "command",
        "command": command
    });
    let new_entry = if use_matcher {
        json!({
            "matcher": "*",
            "hooks": [hook_item]
        })
    } else {
        json!({
            "hooks": [hook_item]
        })
    };

    if let Some(pos) = entries.iter().position(is_anvil_claude_entry) {
        entries[pos] = new_entry;
    } else {
        entries.push(new_entry);
    }
}

fn is_anvil_claude_entry(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|arr| {
            arr.iter().any(|h| {
                h.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|cmd| cmd.starts_with("anvil hook "))
            })
        })
}
