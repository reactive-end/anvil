//! Conflict-free team task ledger and consolidated `tasks.json` builder.

pub mod active;
pub mod builder;
pub mod identity;
pub mod ledger;
pub mod merge_driver;
pub mod model;

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use anvil_core::fs_util::atomic_write;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use active::{
    ActiveTaskState, activate_task_from_meta, active_task_path, ensure_task_for_prompt,
    load_active_task, mark_summary_recorded, save_active_task,
};
pub use builder::build_root_tasks_json;
pub use identity::{resolve_author, resolve_author_fast};
pub use ledger::{
    append_entry, append_entry_as, list_tasks, start_task, start_task_as, write_task_plan,
};
pub use merge_driver::{install_git_merge_driver, resolve_merge};
pub use model::{ConsolidatedTask, ConsolidatedTasksFile, TaskEntry, TaskMeta, TaskStatus};

/// Serializes `value` as pretty JSON with a trailing newline and writes it atomically.
pub(crate) fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut json = serde_json::to_string_pretty(value).context("failed to serialize JSON")?;
    json.push('\n');
    atomic_write(path, json.as_bytes())
}

/// Reads and deserializes a JSON file from `path`.
pub(crate) fn read_json_file<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse JSON at {}", path.display()))
}
