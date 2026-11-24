//! Domain data structures for the conflict-free task ledger and consolidated `tasks.json`.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Lifecycle state of a tracked task.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    NotStarted,
    #[default]
    InProgress,
    Blocked,
    Done,
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotStarted => f.write_str("not_started"),
            Self::InProgress => f.write_str("in_progress"),
            Self::Blocked => f.write_str("blocked"),
            Self::Done => f.write_str("done"),
        }
    }
}

/// Immutable task creation metadata stored at `.harness/tasks/<task_id>/meta.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskMeta {
    pub task_id: String,
    pub created_by: String,
    pub request: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Immutable progress entry stored at `.harness/tasks/<task_id>/entries/<author>-<ulid>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEntry {
    pub entry_id: String,
    pub task_id: String,
    pub author: String,
    pub status: TaskStatus,
    pub branch: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    #[serde(default)]
    pub files_touched: Vec<String>,
    pub timestamp: DateTime<Utc>,
}

/// Consolidated view of a single task aggregated from `meta.json` and all `entries/*.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsolidatedTask {
    pub task_id: String,
    pub request: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    pub status: TaskStatus,
    pub created_by: String,
    pub contributors: Vec<String>,
    pub branches: Vec<String>,
    pub files_touched: Vec<String>,
    pub summary: String,
    pub entry_count: usize,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub entries: Vec<TaskEntry>,
}

/// Root `tasks.json` file deterministically rebuilt from `.harness/tasks/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsolidatedTasksFile {
    pub version: u32,
    pub task_count: usize,
    pub updated_at: Option<DateTime<Utc>>,
    pub tasks: Vec<ConsolidatedTask>,
}

/// Cached author identity stored at `.harness/.state/identity.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityCache {
    pub author: String,
    pub source: String,
    pub resolved_at: DateTime<Utc>,
}
