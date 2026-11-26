//! Unified internal hook contract shared between Antigravity and Claude Code adapters.

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Supported AI coding harness adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    Antigravity,
    ClaudeCode,
}

impl AdapterKind {
    /// Parses CLI/config adapter identifiers such as `antigravity`, `claude`, or `claude_code`.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "antigravity" | "agy" => Ok(Self::Antigravity),
            "claude" | "claude_code" | "claude-code" => Ok(Self::ClaudeCode),
            other => bail!("unsupported adapter kind: {other}"),
        }
    }
}

/// Hook lifecycle events normalized across adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEvent {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    PreInvocation,
    Stop,
}

impl HookEvent {
    /// Parses PascalCase, snake_case, or kebab-case hook event names.
    pub fn parse(raw: &str) -> Result<Self> {
        let normalized: String = raw
            .trim()
            .chars()
            .filter(|c| *c != '_' && *c != '-')
            .flat_map(char::to_lowercase)
            .collect();
        match normalized.as_str() {
            "sessionstart" => Ok(Self::SessionStart),
            "userpromptsubmit" => Ok(Self::UserPromptSubmit),
            "pretooluse" => Ok(Self::PreToolUse),
            "posttooluse" => Ok(Self::PostToolUse),
            "preinvocation" => Ok(Self::PreInvocation),
            "stop" => Ok(Self::Stop),
            _ => bail!("unsupported hook event: {raw}"),
        }
    }

    /// Returns the canonical PascalCase event name used in hook config files.
    pub fn as_pascal_case(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PreInvocation => "PreInvocation",
            Self::Stop => "Stop",
        }
    }
}

/// Normalized input parsed from either Antigravity or Claude Code hook stdin JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedHookInput {
    pub adapter: AdapterKind,
    pub event: HookEvent,
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    /// Subagent id; absent on the main thread.
    pub agent_id: Option<String>,
    /// Agent type of the subagent, or of a main thread started with `--agent`.
    pub agent_type: Option<String>,
    /// Transcript of the main session (`<dir>/<session_id>.jsonl`); subagents keep theirs beside it.
    #[serde(default)]
    pub transcript_path: Option<String>,
    /// Id of this tool call; an `Agent` call's id names the subagent it starts.
    #[serde(default)]
    pub tool_use_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<Value>,
    pub tool_output: Option<Value>,
    pub prompt: Option<String>,
    pub user_prompt: Option<String>,
}

/// Severity level for messages queued in `.harness/.state/inbox.json` or injected into context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageLevel {
    Info,
    Warning,
    Error,
}

/// A diagnostic or context message passed from `PostToolUse` to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxMessage {
    pub source: String,
    pub level: MessageLevel,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl InboxMessage {
    /// Creates a timestamped inbox message.
    pub fn new(
        source: impl Into<String>,
        level: MessageLevel,
        content: impl Into<String>,
        file: Option<String>,
    ) -> Self {
        Self {
            source: source.into(),
            level,
            content: content.into(),
            file,
            created_at: Utc::now(),
        }
    }
}

/// Permission decision for `PreToolUse` hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionAction {
    Allow,
    Deny,
    Ask,
    Overwrite,
}

/// Adapter-agnostic outcome returned by `anvil hook` handlers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookOutcome {
    pub action: PermissionAction,
    pub block_stop: bool,
    pub reason: Option<String>,
    pub updated_input: Option<Value>,
    pub messages: Vec<InboxMessage>,
}

impl Default for HookOutcome {
    fn default() -> Self {
        Self::allow()
    }
}

impl HookOutcome {
    /// Allows the action with no extra context or messages.
    pub fn allow() -> Self {
        Self {
            action: PermissionAction::Allow,
            block_stop: false,
            reason: None,
            updated_input: None,
            messages: Vec::new(),
        }
    }

    /// Denies a `PreToolUse` invocation with a structured reason.
    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            action: PermissionAction::Deny,
            reason: Some(reason.into()),
            ..Self::allow()
        }
    }

    /// Prompts the user for confirmation on a `PreToolUse` invocation.
    pub fn ask(reason: impl Into<String>) -> Self {
        Self {
            action: PermissionAction::Ask,
            reason: Some(reason.into()),
            ..Self::allow()
        }
    }

    /// Overwrites the `PreToolUse` tool input (for example wrapping a command with `anvil run`).
    pub fn overwrite(updated_input: Value, reason: Option<String>) -> Self {
        Self {
            action: PermissionAction::Overwrite,
            reason,
            updated_input: Some(updated_input),
            ..Self::allow()
        }
    }

    /// Blocks a `Stop` hook so the agent resolves failing gates first.
    pub fn block_stop(reason: impl Into<String>) -> Self {
        Self {
            block_stop: true,
            reason: Some(reason.into()),
            ..Self::allow()
        }
    }

    /// Returns an `Allow` outcome carrying findings/context messages.
    pub fn with_messages(messages: Vec<InboxMessage>) -> Self {
        Self {
            messages,
            ..Self::allow()
        }
    }
}
