//! Text the guard inspects beyond a plain shell command, and its self-protection rules.
//!
//! The agent must not be able to relax its own guard, so changes to the guard configuration always
//! need the user, whatever the configured decisions say.

use std::path::Path;

use anvil_adapters::contract::NormalizedHookInput;
use anvil_core::config::security_guard::{GuardCategory, GuardDecision};
use serde_json::Value;

use super::super::payload::{extract_command_field, extract_rel_file, is_mutating_file_tool};
use super::super::shell::expand::{self, Invocation};

const MCP_SHELL_FIELDS: &[&str] = &["cli_command", "command", "cmd", "script"];
const MCP_QUERY_FIELDS: &[&str] = &["sql", "query", "statement"];
const GUARD_WRITERS: &[&str] = &["set", "add", "remove", "reset"];
const GUARD_CONFIG_KEYS: &[&str] =
    &["security_guard", "deny_patterns", "ask_patterns", "allow_patterns", "\"categories\""];
const CONFIG_PATH: &str = ".harness/config.json";

/// How a piece of tool input is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    /// A shell command line.
    Shell,
    /// SQL, MongoDB, or Redis text sent to a database tool.
    Query,
}

/// A string from the tool input that the guard inspects.
#[derive(Debug, Clone, Copy)]
pub struct GuardText<'a> {
    pub kind: TextKind,
    pub text: &'a str,
}

/// Returns `true` for MCP tools other than Anvil's own server.
pub fn is_mcp_tool(tool_name: Option<&str>) -> bool {
    tool_name.is_some_and(|name| name.starts_with("mcp__") && !name.starts_with("mcp__anvil__"))
}

/// Collects the command text of `input`: the shell command field, and for MCP tools every
/// shell-like and SQL-like top-level string field.
pub fn guard_texts(input: &NormalizedHookInput) -> Vec<GuardText<'_>> {
    if !is_mcp_tool(input.tool_name.as_deref()) {
        return extract_command_field(input.tool_input.as_ref())
            .map(|(_, text)| GuardText { kind: TextKind::Shell, text })
            .into_iter()
            .collect();
    }
    let Some(fields) = input.tool_input.as_ref().and_then(Value::as_object) else {
        return Vec::new();
    };
    let pick = |names: &'static [&'static str], kind: TextKind| {
        names.iter().filter_map(move |name| {
            let text = fields.get(*name)?.as_str()?.trim();
            (!text.is_empty()).then_some(GuardText { kind, text })
        })
    };
    pick(MCP_SHELL_FIELDS, TextKind::Shell).chain(pick(MCP_QUERY_FIELDS, TextKind::Query)).collect()
}

/// Explains why the call touches the guard's own configuration, or `None` when it does not.
pub fn protects_config(
    root: &Path,
    input: &NormalizedHookInput,
    texts: &[GuardText<'_>],
) -> Option<&'static str> {
    let shell_hit = texts
        .iter()
        .filter(|t| t.kind == TextKind::Shell)
        .any(|t| runs_guard_writer(t.text) || shell_edits_config(t.text));
    (shell_hit || file_edits_config(root, input)).then_some("changing the security guard settings")
}

/// Returns `true` when `cmd` runs `anvil guard set|add|remove|reset` in a form the expander sees.
fn runs_guard_writer(cmd: &str) -> bool {
    let expansion = expand::expand(cmd);
    expansion.invocations.iter().any(is_guard_writer)
        || expansion.inline_code.iter().any(|code| names_guard_writer(code))
}

fn is_guard_writer(inv: &Invocation) -> bool {
    matches!(inv.name.as_str(), "anvil" | "cargo")
        && inv.args.windows(2).any(|w| w[0] == "guard" && GUARD_WRITERS.contains(&w[1].as_str()))
}

fn names_guard_writer(code: &str) -> bool {
    let flat = code.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
    GUARD_WRITERS.iter().any(|verb| flat.contains(&format!("anvil guard {verb}")))
}

fn shell_edits_config(cmd: &str) -> bool {
    cmd.replace('\\', "/").contains(CONFIG_PATH) && mentions_guard_config(cmd)
}

fn file_edits_config(root: &Path, input: &NormalizedHookInput) -> bool {
    if !is_mutating_file_tool(input.tool_name.as_deref()) {
        return false;
    }
    let Some(rel) = extract_rel_file(root, input.tool_input.as_ref()) else {
        return false;
    };
    if rel != CONFIG_PATH && !rel.ends_with(&format!("/{CONFIG_PATH}")) {
        return false;
    }
    let mut strings = Vec::new();
    if let Some(value) = input.tool_input.as_ref() {
        collect_strings(value, 0, &mut strings);
    }
    strings.iter().any(|s| mentions_guard_config(s))
}

fn collect_strings<'a>(value: &'a Value, depth: usize, out: &mut Vec<&'a str>) {
    if depth > 4 {
        return;
    }
    match value {
        Value::String(s) => out.push(s),
        Value::Array(items) => items.iter().for_each(|v| collect_strings(v, depth + 1, out)),
        Value::Object(map) => map.values().for_each(|v| collect_strings(v, depth + 1, out)),
        _ => {}
    }
}

/// Returns `true` when `text` names the guard section or looks like an edit of a decision.
fn mentions_guard_config(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if GUARD_CONFIG_KEYS.iter().any(|key| lower.contains(key)) {
        return true;
    }
    let quoted = |word: &str| lower.contains(&format!("\"{word}\""));
    GuardDecision::ALL.iter().any(|d| quoted(d.as_str()))
        && GuardCategory::ALL.iter().any(|c| quoted(c.as_str()))
}
