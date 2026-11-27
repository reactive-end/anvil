//! `search` section of `.harness/config.json`: which engine answers `anvil grep` and the MCP `grep` tool.

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

/// Repository search settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SearchConfig {
    #[serde(default)]
    pub backend: SearchBackend,
}
