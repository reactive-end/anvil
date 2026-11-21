//! Non-destructive generator and merger for `.agents/hooks.json` and `.agents/mcp_config.json`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use crate::contract::HookEvent;
use crate::sync::write_if_changed;

const ANTIGRAVITY_EVENTS: &[HookEvent] = &[
    HookEvent::SessionStart,
    HookEvent::PreToolUse,
    HookEvent::PostToolUse,
    HookEvent::PreInvocation,
    HookEvent::Stop,
];

/// Returns the path to `.agents/hooks.json`.
pub fn hooks_config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".agents").join("hooks.json")
}

/// Returns the path to `.agents/mcp_config.json`.
pub fn mcp_config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".agents").join("mcp_config.json")
}

/// Non-destructively merges `anvil hook` handlers into `.agents/hooks.json`.
pub fn merge_hooks_config(repo_root: &Path) -> Result<bool> {
    let path = hooks_config_path(repo_root);
    let mut root = read_json_object(&path)?;

    let hooks_obj = ensure_object_field(&mut root, "hooks")?;
    for event in ANTIGRAVITY_EVENTS {
        let key = event.as_pascal_case();
        let command = format!("anvil hook {key} --adapter antigravity");
        let list = ensure_array_field(hooks_obj, key)?;
        upsert_anvil_hook_entry(list, &command);
    }

    let formatted = serde_json::to_string_pretty(&Value::Object(root))
        .context("failed to serialize .agents/hooks.json")?;
    write_if_changed(&path, &format!("{formatted}\n"))
}

/// Non-destructively merges the `anvil mcp` server entry into `.agents/mcp_config.json`.
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
        .context("failed to serialize .agents/mcp_config.json")?;
    write_if_changed(&path, &format!("{formatted}\n"))
}

fn upsert_anvil_hook_entry(entries: &mut Vec<Value>, command: &str) {
    let new_entry = json!({ "command": command });
    if let Some(pos) = entries.iter().position(is_anvil_hook_entry) {
        entries[pos] = new_entry;
    } else {
        entries.push(new_entry);
    }
}

fn is_anvil_hook_entry(entry: &Value) -> bool {
    entry
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|cmd| cmd.starts_with("anvil hook "))
}

pub(crate) fn read_json_object(path: &Path) -> Result<Map<String, Value>> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(Map::new());
    }
    let val: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse JSON in {}", path.display()))?;
    match val {
        Value::Object(map) => Ok(map),
        _ => anyhow::bail!("expected JSON object at {}", path.display()),
    }
}

pub(crate) fn ensure_object_field<'a>(
    map: &'a mut Map<String, Value>,
    key: &str,
) -> Result<&'a mut Map<String, Value>> {
    if !map.contains_key(key) {
        map.insert(key.to_owned(), Value::Object(Map::new()));
    }
    map.get_mut(key)
        .and_then(Value::as_object_mut)
        .with_context(|| format!("expected '{key}' to be a JSON object"))
}

pub(crate) fn ensure_array_field<'a>(
    map: &'a mut Map<String, Value>,
    key: &str,
) -> Result<&'a mut Vec<Value>> {
    if !map.contains_key(key) {
        map.insert(key.to_owned(), Value::Array(Vec::new()));
    }
    map.get_mut(key)
        .and_then(Value::as_array_mut)
        .with_context(|| format!("expected '{key}' to be a JSON array"))
}
