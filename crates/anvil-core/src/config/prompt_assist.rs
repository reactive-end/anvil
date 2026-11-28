//! `prompt_assist` section of `.harness/config.json`: the small context block (and optional coaching)
//! the `UserPromptSubmit` hook adds to every prompt. Deterministic: no model call, no network.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Tokens (about four characters each) the whole injected block may use unless configured.
pub const DEFAULT_MAX_TOKENS: usize = 450;
/// Prompts shorter than this many words get nothing: they are follow-ups such as "yes, do it".
pub const DEFAULT_MIN_WORDS: usize = 4;

/// What the hook adds to a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptAssistMode {
    /// Nothing.
    Off,
    /// Code symbols, docs concepts, and memories that look relevant to the prompt.
    #[default]
    Context,
    /// The context block, plus a nudge to clarify before editing when the request lacks a target,
    /// success criteria, or a verification hint.
    Coach,
}

impl PromptAssistMode {
    /// Returns the config and CLI spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Context => "context",
            Self::Coach => "coach",
        }
    }

    /// Parses `off`, `context`, or `coach` (case-insensitive).
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "context" => Ok(Self::Context),
            "coach" => Ok(Self::Coach),
            other => bail!("unsupported prompt assist mode `{other}` (use off, context, or coach)"),
        }
    }
}

/// Settings of the prompt assist hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptAssistConfig {
    #[serde(default)]
    pub mode: PromptAssistMode,
    /// Cap for the whole injected block, estimated at four characters per token.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,
    /// Prompts with fewer words are left alone.
    #[serde(default = "default_min_words")]
    pub min_words: usize,
}

impl Default for PromptAssistConfig {
    fn default() -> Self {
        Self {
            mode: PromptAssistMode::default(),
            max_tokens: DEFAULT_MAX_TOKENS,
            min_words: DEFAULT_MIN_WORDS,
        }
    }
}

impl PromptAssistConfig {
    /// `true` when nothing was changed, so the section is left out of the saved file.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

fn default_max_tokens() -> usize {
    DEFAULT_MAX_TOKENS
}

fn default_min_words() -> usize {
    DEFAULT_MIN_WORDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_context_with_a_450_token_cap() {
        let cfg: PromptAssistConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.mode, PromptAssistMode::Context);
        assert_eq!((cfg.max_tokens, cfg.min_words), (450, 4));
        assert!(cfg.is_default());
    }

    #[test]
    fn every_mode_round_trips_through_its_spelling() {
        for mode in [PromptAssistMode::Off, PromptAssistMode::Context, PromptAssistMode::Coach] {
            assert_eq!(PromptAssistMode::parse(mode.as_str()).unwrap(), mode);
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, format!("\"{}\"", mode.as_str()));
        }
        assert!(PromptAssistMode::parse("loud").is_err());
    }

    #[test]
    fn a_changed_section_is_not_default() {
        let cfg = PromptAssistConfig { mode: PromptAssistMode::Coach, ..Default::default() };
        assert!(!cfg.is_default());
    }
}
