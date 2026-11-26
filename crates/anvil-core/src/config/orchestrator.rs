//! Orchestrator/executor model pair configuration.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

const MODEL_ALIASES: &[&str] = &["fable", "opus", "sonnet", "haiku", "inherit"];

/// Reasoning effort level accepted by Claude Code agent definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortLevel {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl EffortLevel {
    /// Every level from cheapest to most thorough.
    pub const ALL: [Self; 5] = [Self::Low, Self::Medium, Self::High, Self::Xhigh, Self::Max];

    /// Returns the frontmatter spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    /// Parses a level name.
    pub fn parse(raw: &str) -> Result<Self> {
        let name = raw.trim().to_ascii_lowercase();
        match Self::ALL.into_iter().find(|level| level.as_str() == name) {
            Some(level) => Ok(level),
            None => bail!("unsupported effort `{raw}` (use low, medium, high, xhigh, or max)"),
        }
    }
}

/// Model and effort assigned to one role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleModel {
    pub model: String,
    pub effort: EffortLevel,
}

/// Pairs a strong planning model with a cheaper execution model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    pub orchestrator: RoleModel,
    pub executor: RoleModel,
    /// Tools the executor gets besides its default set, such as `mcp__godot__*`; invalid names are dropped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub executor_tools: Vec<String>,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            orchestrator: RoleModel {
                model: "opus".to_owned(),
                effort: EffortLevel::High,
            },
            executor: RoleModel {
                model: "sonnet".to_owned(),
                effort: EffortLevel::Medium,
            },
            executor_tools: Vec::new(),
        }
    }
}

/// Accepts Claude Code aliases and full `claude-*` model IDs; rejects anything that could break
/// out of the generated YAML frontmatter.
pub fn validate_model(raw: &str) -> Result<String> {
    let name = raw.trim();
    let is_full_id = name.starts_with("claude-")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '[' | ']'));
    if MODEL_ALIASES.contains(&name) || is_full_id {
        return Ok(name.to_owned());
    }
    bail!(
        "unsupported model `{raw}` (use {} or a full `claude-*` ID)",
        MODEL_ALIASES.join(", ")
    )
}

/// `true` for a Claude Code tool name or MCP pattern (`Read`, `mcp__server__tool`, `mcp__server__*`),
/// so nothing else can reach an agent's frontmatter.
pub fn is_tool_name(raw: &str) -> bool {
    let body = raw.strip_suffix("__*").unwrap_or(raw);
    let mut chars = body.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}
