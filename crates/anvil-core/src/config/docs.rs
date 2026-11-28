//! `docs` section of `.harness/config.json`: living documentation in the Open Knowledge Format.

use serde::{Deserialize, Serialize};

/// Settings of `anvil docs` and its hooks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocsConfig {
    /// Project name of the bundle; defaults to the repository folder name (or the subrepo name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Remind the agent to update a concept after it edits a file the concept documents.
    #[serde(default = "default_remind_on_edit")]
    pub remind_on_edit: bool,
}

impl Default for DocsConfig {
    fn default() -> Self {
        Self { project: None, remind_on_edit: default_remind_on_edit() }
    }
}

fn default_remind_on_edit() -> bool {
    true
}
