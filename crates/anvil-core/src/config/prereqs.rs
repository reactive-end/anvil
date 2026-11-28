//! `prerequisites` section of `.harness/config.json`: what a machine needs to work on the project.
//!
//! `detected` is rewritten by `anvil prereqs detect --write`; `extra_tools`, `ignore` and `custom` are the
//! user's overrides and are never touched by detection. Subrepos of a monorepo share one flat list: every
//! entry names its subrepo, which keeps `SubrepoConfig` (rebuilt on every detection) free of user data.

use serde::{Deserialize, Serialize};

/// A command-line tool (or runtime) that must be installed, optionally at a given version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPrereq {
    pub name: String,
    /// Version constraint: npm range, Python specifier, Terraform constraint, or a Java major.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Where the requirement came from, e.g. `orders-api/package.json engines.node`.
    #[serde(default)]
    pub source: String,
    /// Subrepo the requirement belongs to; `None` for the workspace root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subrepo: Option<String>,
    /// Failing a required tool fails `anvil prereqs check`; an optional one only warns.
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
}

impl ToolPrereq {
    /// A required tool without a version constraint.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: None,
            source: String::new(),
            subrepo: None,
            required: true,
            hint: String::new(),
        }
    }
}

/// A real env file that must define every key of its example file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvFilePrereq {
    /// Repo-relative path of the real file, e.g. `orders-api/.env`.
    pub file: String,
    /// Repo-relative path of the example, e.g. `orders-api/.env.example`.
    pub example: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subrepo: Option<String>,
}

/// An environment variable that must be set (or defined in the project's `.env`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVarPrereq {
    pub name: String,
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subrepo: Option<String>,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
}

/// A file or directory that must exist, such as `.venv` or `node_modules`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathPrereq {
    /// Repo-relative path.
    pub path: String,
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subrepo: Option<String>,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
}

/// A user-defined check: `command` runs through the shell under a timeout and exit 0 means ok.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomPrereq {
    pub name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subrepo: Option<String>,
    #[serde(default = "default_true")]
    pub required: bool,
}

/// Everything a project needs on a machine.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PrereqSet {
    #[serde(default)]
    pub tools: Vec<ToolPrereq>,
    #[serde(default)]
    pub env_files: Vec<EnvFilePrereq>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_vars: Vec<EnvVarPrereq>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<PathPrereq>,
    /// Names of the servers declared in `.mcp.json`; the check re-reads the files themselves.
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    #[serde(default)]
    pub custom: Vec<CustomPrereq>,
}

/// The `prerequisites` config section: the detected set plus the user's overrides.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PrerequisitesConfig {
    #[serde(default)]
    pub detected: PrereqSet,
    /// Tools the user adds on top of the detected ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_tools: Vec<ToolPrereq>,
    /// Names (tools, env vars, MCP servers, custom checks) dropped from the effective set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom: Vec<CustomPrereq>,
}

impl PrerequisitesConfig {
    /// True when nothing was detected and no override exists (the section is then not serialised).
    pub fn is_empty(&self) -> bool {
        self.detected == PrereqSet::default()
            && self.extra_tools.is_empty()
            && self.ignore.is_empty()
            && self.custom.is_empty()
    }

    /// The detected set with the user's overrides applied.
    pub fn effective(&self) -> PrereqSet {
        self.with_detected(self.detected.clone())
    }

    /// `detected` (typically fresh from detection) with the user's overrides applied.
    pub fn with_detected(&self, mut set: PrereqSet) -> PrereqSet {
        let ignored = |name: &str| self.ignore.iter().any(|i| i.eq_ignore_ascii_case(name));
        set.tools.retain(|t| !ignored(&t.name));
        set.env_vars.retain(|v| !ignored(&v.name));
        set.mcp_servers.retain(|s| !ignored(s));
        set.custom.retain(|c| !ignored(&c.name));
        set.tools.extend(self.extra_tools.iter().filter(|t| !ignored(&t.name)).cloned());
        set.custom.extend(self.custom.iter().filter(|c| !ignored(&c.name)).cloned());
        set
    }
}

fn default_true() -> bool {
    true
}
