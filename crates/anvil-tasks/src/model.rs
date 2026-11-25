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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub done_steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocker: Option<String>,
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

/// Schema version of the lightweight root `tasks.json` index.
pub const TASKS_INDEX_SCHEMA_VERSION: u32 = 2;

/// Lightweight summary row persisted in root `tasks.json` (full history lives under `.harness/tasks/<ID>/`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskIndexEntry {
    pub task_id: String,
    pub request: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub status: TaskStatus,
    pub created_by: String,
    #[serde(default)]
    pub contributors: Vec<String>,
    #[serde(default)]
    pub branches: Vec<String>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    #[serde(default)]
    pub entry_count: usize,
    #[serde(default)]
    pub files_count: usize,
    pub progress_file: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&ConsolidatedTask> for TaskIndexEntry {
    fn from(task: &ConsolidatedTask) -> Self {
        Self {
            task_id: task.task_id.clone(),
            request: task.request.clone(),
            project: task.project.clone(),
            status: task.status,
            created_by: task.created_by.clone(),
            contributors: task.contributors.clone(),
            branches: task.branches.clone(),
            summary: task.summary.clone(),
            evidence: task.evidence.clone(),
            entry_count: task.entry_count,
            files_count: task.files_touched.len(),
            progress_file: crate::progress::task_progress_rel_path(&task.task_id),
            created_at: task.created_at,
            updated_at: task.updated_at,
        }
    }
}

/// Compact root `tasks.json` index rebuilt from `.harness/tasks/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TasksIndexFile {
    pub version: u32,
    pub task_count: usize,
    pub updated_at: Option<DateTime<Utc>>,
    pub tasks: Vec<TaskIndexEntry>,
}

impl From<&ConsolidatedTasksFile> for TasksIndexFile {
    fn from(file: &ConsolidatedTasksFile) -> Self {
        Self {
            version: TASKS_INDEX_SCHEMA_VERSION,
            task_count: file.task_count,
            updated_at: file.updated_at,
            tasks: file.tasks.iter().map(TaskIndexEntry::from).collect(),
        }
    }
}

/// Full in-memory consolidated view (and legacy v1 format) from `meta.json` + `entries/*.json`.
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
