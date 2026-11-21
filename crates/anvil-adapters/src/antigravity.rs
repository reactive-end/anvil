//! Antigravity hook JSON translator and config generators.

pub mod config;

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::contract::{AdapterKind, HookEvent, HookOutcome, NormalizedHookInput, PermissionAction};
use crate::inbox;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAntigravityInput {
    #[serde(alias = "hook_event_name")]
    hook_event_name: Option<String>,
    cwd: Option<String>,
    #[serde(alias = "session_id")]
    session_id: Option<String>,
    prompt: Option<String>,
    #[serde(alias = "tool_call")]
    tool_call: Option<RawToolCall>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawToolCall {
    #[serde(alias = "name", alias = "tool_name")]
    tool_name: Option<String>,
    #[serde(alias = "input", alias = "arguments", alias = "tool_input")]
    tool_input: Option<Value>,
    #[serde(alias = "output", alias = "response", alias = "tool_output")]
    tool_output: Option<Value>,
}

/// Parses Antigravity camelCase hook stdin JSON into a normalized hook input.
pub fn parse_input(raw_json: &str, event_hint: Option<HookEvent>) -> Result<NormalizedHookInput> {
    let raw: RawAntigravityInput = if raw_json.trim().is_empty() {
        RawAntigravityInput::default()
    } else {
        serde_json::from_str(raw_json).context("failed to parse Antigravity hook stdin JSON")?
    };

    let event = match (&raw.hook_event_name, event_hint) {
        (Some(name), _) => HookEvent::parse(name)?,
        (None, Some(hint)) => hint,
        (None, None) => anyhow::bail!("missing hookEventName in Antigravity hook input"),
    };

    let tool_call = raw.tool_call.unwrap_or_default();
    Ok(NormalizedHookInput {
        adapter: AdapterKind::Antigravity,
        event,
        cwd: raw.cwd,
        session_id: raw.session_id,
        tool_name: tool_call.tool_name,
        tool_input: tool_call.tool_input,
        tool_output: tool_call.tool_output,
        prompt: raw.prompt,
    })
}

/// Renders a normalized `HookOutcome` into Antigravity's stdout JSON contract.
///
/// For `PostToolUse`, findings are queued into `.harness/.state/inbox.json` and `{}` is returned.
/// For `PreInvocation`, queued inbox messages are drained and emitted via `injectSteps`.
pub fn render_output(
    repo_root: &Path,
    event: HookEvent,
    outcome: &HookOutcome,
) -> Result<Value> {
    match event {
        HookEvent::PreToolUse => Ok(render_pre_tool_use(outcome)),
        HookEvent::PostToolUse => {
            inbox::push_messages(repo_root, &outcome.messages)?;
            Ok(json!({}))
        }
        HookEvent::SessionStart => Ok(render_inject_steps(&outcome.messages)),
        HookEvent::PreInvocation => {
            let mut combined = inbox::drain_inbox(repo_root)?;
            combined.extend(outcome.messages.iter().cloned());
            Ok(render_inject_steps(&combined))
        }
        HookEvent::Stop => Ok(render_stop(outcome)),
    }
}

fn render_pre_tool_use(outcome: &HookOutcome) -> Value {
    let mut obj = Map::new();
    let decision = match outcome.action {
        PermissionAction::Allow | PermissionAction::Overwrite => "allow",
        PermissionAction::Deny => "deny",
        PermissionAction::Ask => "ask",
    };
    obj.insert("decision".to_owned(), Value::String(decision.to_owned()));

    if let Some(reason) = &outcome.reason {
        obj.insert("reason".to_owned(), Value::String(reason.clone()));
    }
    if outcome.action == PermissionAction::Overwrite
        && let Some(overwrite) = &outcome.updated_input
    {
        obj.insert("overwrite".to_owned(), overwrite.clone());
    }
    Value::Object(obj)
}

fn render_inject_steps(messages: &[crate::contract::InboxMessage]) -> Value {
    let Some(text) = inbox::format_context(messages) else {
        return json!({});
    };
    json!({
        "injectSteps": [
            {
                "systemMessage": {
                    "systemMessage": text
                }
            }
        ]
    })
}

fn render_stop(outcome: &HookOutcome) -> Value {
    let mut obj = Map::new();
    let decision = if outcome.block_stop {
        "continue"
    } else {
        "allow"
    };
    obj.insert("decision".to_owned(), Value::String(decision.to_owned()));
    if let Some(reason) = &outcome.reason {
        obj.insert("reason".to_owned(), Value::String(reason.clone()));
    }
    Value::Object(obj)
}
