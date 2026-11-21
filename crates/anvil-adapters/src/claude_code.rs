//! Claude Code hook JSON translator and config mergers.

pub mod config;

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::contract::{AdapterKind, HookEvent, HookOutcome, NormalizedHookInput, PermissionAction};
use crate::inbox;

#[derive(Debug, Default, Deserialize)]
struct RawClaudeInput {
    #[serde(alias = "hookEventName")]
    hook_event_name: Option<String>,
    cwd: Option<String>,
    #[serde(alias = "sessionId")]
    session_id: Option<String>,
    #[serde(alias = "toolName")]
    tool_name: Option<String>,
    #[serde(alias = "toolInput")]
    tool_input: Option<Value>,
    #[serde(alias = "tool_output", alias = "toolOutput", alias = "toolResponse")]
    tool_response: Option<Value>,
    prompt: Option<String>,
}

/// Parses Claude Code hook stdin JSON into a normalized hook input.
pub fn parse_input(raw_json: &str, event_hint: Option<HookEvent>) -> Result<NormalizedHookInput> {
    let raw: RawClaudeInput = if raw_json.trim().is_empty() {
        RawClaudeInput::default()
    } else {
        serde_json::from_str(raw_json).context("failed to parse Claude Code hook stdin JSON")?
    };

    let event = match (&raw.hook_event_name, event_hint) {
        (Some(name), _) => HookEvent::parse(name)?,
        (None, Some(hint)) => hint,
        (None, None) => anyhow::bail!("missing hook_event_name in Claude Code hook input"),
    };

    Ok(NormalizedHookInput {
        adapter: AdapterKind::ClaudeCode,
        event,
        cwd: raw.cwd,
        session_id: raw.session_id,
        tool_name: raw.tool_name,
        tool_input: raw.tool_input,
        tool_output: raw.tool_response,
        prompt: raw.prompt,
    })
}

/// Renders a normalized `HookOutcome` into Claude Code's stdout JSON contract.
///
/// For `PostToolUse`, returns `additionalContext` directly and clears `.harness/.state/inbox.json`.
pub fn render_output(
    repo_root: &Path,
    event: HookEvent,
    outcome: &HookOutcome,
) -> Result<Value> {
    match event {
        HookEvent::PreToolUse => Ok(render_pre_tool_use(outcome)),
        HookEvent::PostToolUse => {
            let mut combined = inbox::drain_inbox(repo_root)?;
            combined.extend(outcome.messages.iter().cloned());
            Ok(render_additional_context("PostToolUse", &combined))
        }
        HookEvent::SessionStart => Ok(render_additional_context("SessionStart", &outcome.messages)),
        HookEvent::PreInvocation => Ok(json!({})),
        HookEvent::Stop => Ok(render_stop(outcome)),
    }
}

fn render_pre_tool_use(outcome: &HookOutcome) -> Value {
    let mut specific = Map::new();
    specific.insert(
        "hookEventName".to_owned(),
        Value::String("PreToolUse".to_owned()),
    );

    let decision = match outcome.action {
        PermissionAction::Allow | PermissionAction::Overwrite => "allow",
        PermissionAction::Deny => "deny",
        PermissionAction::Ask => "ask",
    };
    specific.insert(
        "permissionDecision".to_owned(),
        Value::String(decision.to_owned()),
    );

    if let Some(reason) = &outcome.reason {
        specific.insert(
            "permissionDecisionReason".to_owned(),
            Value::String(reason.clone()),
        );
    }
    if outcome.action == PermissionAction::Overwrite
        && let Some(updated) = &outcome.updated_input
    {
        specific.insert("updatedInput".to_owned(), updated.clone());
    }

    json!({ "hookSpecificOutput": Value::Object(specific) })
}

fn render_additional_context(
    event_name: &str,
    messages: &[crate::contract::InboxMessage],
) -> Value {
    let Some(text) = inbox::format_context(messages) else {
        return json!({});
    };
    json!({
        "hookSpecificOutput": {
            "hookEventName": event_name,
            "additionalContext": text
        }
    })
}

fn render_stop(outcome: &HookOutcome) -> Value {
    if !outcome.block_stop {
        return json!({});
    }
    let mut obj = Map::new();
    obj.insert("decision".to_owned(), Value::String("block".to_owned()));
    if let Some(reason) = &outcome.reason {
        obj.insert("reason".to_owned(), Value::String(reason.clone()));
    }
    Value::Object(obj)
}
