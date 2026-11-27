//! `db` section of `.harness/config.json`: the Google MCP Toolbox for Databases setup that scenarios
//! and agents use to look at the application database.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Toolbox binary, its tools file, and whether agents get it as an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbConfig {
    /// A bare name looked up on `PATH`, or a path relative to the repository root.
    #[serde(default = "default_toolbox", skip_serializing_if = "is_default_toolbox")]
    pub toolbox: String,
    /// Toolbox tools file (relative to the repository root). Toolbox always runs from the root,
    /// so relative SQLite paths inside it are written relative to the root too.
    #[serde(default = "default_config", skip_serializing_if = "is_default_config")]
    pub config: String,
    /// Register the tools file as the `anvil-db` MCP server in every adapter.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mcp: bool,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self { toolbox: default_toolbox(), config: default_config(), mcp: false }
    }
}

impl DbConfig {
    /// `true` when nothing was configured, so the section is left out of the saved file.
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    /// Absolute location of the tools file.
    pub fn config_path(&self, root: &Path) -> PathBuf {
        root.join(&self.config)
    }

    /// The program to run: the bare name for `PATH` lookup, or a path resolved against `root`.
    pub fn toolbox_program(&self, root: &Path) -> PathBuf {
        let has_separator = self.toolbox.contains(['/', '\\']);
        if has_separator { root.join(&self.toolbox) } else { PathBuf::from(&self.toolbox) }
    }
}

fn default_toolbox() -> String {
    "toolbox".to_owned()
}

fn default_config() -> String {
    ".harness/db/tools.yaml".to_owned()
}

fn is_default_toolbox(value: &String) -> bool {
    value == "toolbox"
}

fn is_default_config(value: &String) -> bool {
    value == ".harness/db/tools.yaml"
}

fn is_false(flag: &bool) -> bool {
    !flag
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_left_out_and_restored() {
        let json = serde_json::to_value(DbConfig::default()).expect("serialize");
        assert_eq!(json, serde_json::json!({}));
        let parsed: DbConfig = serde_json::from_str("{}").expect("parse");
        assert!(parsed.is_default());
        let set: DbConfig = serde_json::from_str(r#"{"mcp":true}"#).expect("parse");
        assert!(set.mcp && set.toolbox == "toolbox");
    }

    #[test]
    fn bare_names_use_path_and_separators_mean_repo_relative() {
        let root = Path::new("/repo");
        assert_eq!(DbConfig::default().toolbox_program(root), PathBuf::from("toolbox"));
        let local = DbConfig { toolbox: ".tools/toolbox.exe".to_owned(), ..DbConfig::default() };
        assert_eq!(local.toolbox_program(root), root.join(".tools/toolbox.exe"));
    }
}
