//! Result of a prerequisites check: one [`Item`] per requirement, serialisable as `--json`.

use serde::Serialize;

/// Outcome of one requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Missing,
    WrongVersion,
    Warning,
}

/// What kind of requirement an [`Item`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Tool,
    EnvFile,
    EnvVar,
    Path,
    Mcp,
    Secret,
    Custom,
}

/// Group label of items that belong to the workspace root rather than to a subrepo.
pub const ROOT_GROUP: &str = "(root)";

/// One checked requirement. Values of env files, env vars and credentials never appear in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub group: String,
    pub subrepo: Option<String>,
    pub kind: Kind,
    pub name: String,
    pub status: Status,
    pub found: Option<String>,
    pub constraint: Option<String>,
    pub source: String,
    pub required: bool,
    pub message: Option<String>,
    pub hint: Option<String>,
}

impl Item {
    pub fn new(subrepo: Option<&str>, kind: Kind, name: &str, status: Status) -> Self {
        Self {
            group: subrepo.unwrap_or(ROOT_GROUP).to_owned(),
            subrepo: subrepo.map(str::to_owned),
            kind,
            name: name.to_owned(),
            status,
            found: None,
            constraint: None,
            source: String::new(),
            required: true,
            message: None,
            hint: None,
        }
    }

    pub fn found(mut self, found: Option<String>) -> Self {
        self.found = found;
        self
    }

    pub fn constraint(mut self, constraint: Option<&str>) -> Self {
        self.constraint = constraint.map(str::to_owned);
        self
    }

    pub fn source(mut self, source: &str) -> Self {
        self.source = source.to_owned();
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Sets the hint, ignoring an empty one.
    pub fn hint(mut self, hint: &str) -> Self {
        self.hint = (!hint.is_empty()).then(|| hint.to_owned());
        self
    }

    /// True for a required item that is missing or at the wrong version.
    pub fn is_failure(&self) -> bool {
        self.required && matches!(self.status, Status::Missing | Status::WrongVersion)
    }
}

/// Counts per status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub ok: usize,
    pub missing: usize,
    pub wrong_version: usize,
    pub warning: usize,
}

/// The whole check: `ok` is false when any required item failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub ok: bool,
    pub summary: Summary,
    pub items: Vec<Item>,
}

impl Report {
    pub fn new(items: Vec<Item>) -> Self {
        let mut summary = Summary::default();
        for item in &items {
            match item.status {
                Status::Ok => summary.ok += 1,
                Status::Missing => summary.missing += 1,
                Status::WrongVersion => summary.wrong_version += 1,
                Status::Warning => summary.warning += 1,
            }
        }
        let ok = !items.iter().any(Item::is_failure);
        Self { ok, summary, items }
    }

    /// Number of required items that are missing or at the wrong version.
    pub fn failures(&self) -> usize {
        self.items.iter().filter(|i| i.is_failure()).count()
    }

    /// Names of the groups in first-seen order.
    pub fn groups(&self) -> Vec<&str> {
        let mut groups: Vec<&str> = Vec::new();
        for item in &self.items {
            if !groups.contains(&item.group.as_str()) {
                groups.push(&item.group);
            }
        }
        groups
    }
}
