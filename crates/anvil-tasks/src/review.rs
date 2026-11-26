//! Review packets and the close check for tasks, on top of `anvil_core::review_gate`.

use std::path::Path;

use anvil_core::review_gate;
use anyhow::{Context, Result};

use crate::active::load_active_task;
use crate::builder::read_single_task;
use crate::files::review_files;

/// A packet built for one task.
pub struct TaskPacket {
    pub task_id: String,
    pub text: String,
    pub files: Vec<String>,
}

/// The explicit task id, or the active task's when none is given.
pub fn resolve_task_id(repo: &Path, explicit: Option<&str>) -> Result<String> {
    if let Some(id) = explicit.map(str::trim).filter(|id| !id.is_empty()) {
        return Ok(id.to_owned());
    }
    load_active_task(repo)?
        .map(|active| active.task_id)
        .context("no task id given and no active task: pass the task id, for example `T-012`")
}

/// Builds the review packet of a task's current change and records it for approval.
pub fn packet(repo: &Path, task_id: Option<&str>) -> Result<TaskPacket> {
    let task_id = resolve_task_id(repo, task_id)?;
    let task = read_single_task(repo, &task_id)?;
    let files = review_files(repo, &task_id);
    let text = review_gate::build_packet(repo, &task_id, &task.request, &files)?;
    review_gate::record_packet(repo, &task_id, &files)?;
    Ok(TaskPacket { task_id, text, files })
}

/// Refuses to close `task_id` in orchestrator mode until its current change was approved.
pub fn ensure_reviewed(repo: &Path, task_id: &str) -> Result<()> {
    review_gate::ensure_reviewed(repo, task_id, &review_files(repo, task_id))
}
