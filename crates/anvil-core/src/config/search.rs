//! `search` section of `.harness/config.json`: which engine answers `anvil grep` and the MCP `grep` tool, and
//! how strictly the PreToolUse hook enforces the graph -> docs -> code lookup order.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Engine that runs repository text search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchBackend {
    /// Use `tgrep` only when its index is served and watched for this repository, else the built-in engine.
    #[default]
    Auto,
    /// Always use the built-in ripgrep-crate engine.
    Builtin,
    /// Prefer Microsoft `tgrep`; fall back to the built-in engine (with a note) when it fails.
    Tgrep,
}

impl SearchBackend {
    /// Returns the config and CLI spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Builtin => "builtin",
            Self::Tgrep => "tgrep",
        }
    }

    /// Parses `auto`, `builtin`, or `tgrep` (case-insensitive).
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "builtin" => Ok(Self::Builtin),
            "tgrep" => Ok(Self::Tgrep),
            other => bail!("unsupported search backend `{other}` (use auto, builtin, or tgrep)"),
        }
    }
}

/// How the PreToolUse hook enforces the lookup order graph -> docs -> code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchCascadeMode {
    /// Deny the first code search of a turn (per agent) with the graph and docs results attached.
    #[default]
    Enforce,
    /// Never block; queue a one-line advisory with the graph and docs hits instead.
    Advise,
    /// Do not interfere with code searches.
    Off,
}

impl SearchCascadeMode {
    /// Returns the config spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enforce => "enforce",
            Self::Advise => "advise",
            Self::Off => "off",
        }
    }
}

/// Repository search settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SearchConfig {
    #[serde(default)]
    pub backend: SearchBackend,
    /// Enforcement of the graph -> docs -> code lookup order in the PreToolUse hook.
    #[serde(default)]
    pub cascade: SearchCascadeMode,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cascade_defaults_to_enforce_and_parses_every_mode() {
        let default: SearchConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(default.cascade, SearchCascadeMode::Enforce);
        for (raw, mode) in [
            ("enforce", SearchCascadeMode::Enforce),
            ("advise", SearchCascadeMode::Advise),
            ("off", SearchCascadeMode::Off),
        ] {
            let parsed: SearchConfig =
                serde_json::from_str(&format!(r#"{{"cascade":"{raw}"}}"#)).unwrap();
            assert_eq!(parsed.cascade, mode);
            assert_eq!(mode.as_str(), raw);
        }
    }
}
