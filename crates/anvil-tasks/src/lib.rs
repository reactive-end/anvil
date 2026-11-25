//! Conflict-free team task ledger and consolidated `tasks.json` builder.

pub mod active;
pub mod builder;
pub mod curate;
pub mod files;
pub mod identity;
pub mod import_export;
pub mod ledger;
pub mod merge_driver;
pub mod model;
pub mod progress;

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use anvil_core::fs_util::atomic_write;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use active::{
    ActiveTaskState, PendingPrompt, activate_task_from_meta, active_task_path,
    ensure_task_for_prompt, load_active_task, mark_summary_recorded,
    mark_summary_recorded_with_files, materialize_task_on_edit,
    materialize_task_on_edit_with_fallback, pending_prompt_path, record_active_command_outcome,
    record_pending_prompt, record_touched_file, save_active_task, update_active_task_progress,
};
pub use builder::{build_root_tasks_json, consolidate_repo_tasks, read_single_task};
pub use curate::{
    TaskTarget, clear_skipped_active_task, clear_untracked_edits,
    is_conversational_or_trivial_prompt, load_untracked_edits, record_curated_task,
    record_curated_task_in_group, record_curated_task_with_id, record_session_edit,
    record_task_skip, untracked_edits_path, validate_curated_task_title,
};
pub use files::{collect_task_files, is_project_file};
pub use identity::{resolve_author, resolve_author_fast};
pub use import_export::{
    ImportReport, ImportedFeature, ImportedFeatureList, export_feature_list, import_feature_list,
    import_feature_list_in_group, parse_feature_list_json,
};
pub use ledger::{
    append_entry, append_entry_as, append_entry_with_details, append_entry_with_evidence,
    compare_task_ids, list_task_groups, list_tasks, migrate_legacy_task_ids, sanitize_group_name,
    start_task, start_task_as, start_task_in_group, start_task_with_id, validate_task_id_path,
    write_task_plan,
};
pub use merge_driver::{install_git_merge_driver, resolve_merge};
pub use model::{
    ConsolidatedTask, ConsolidatedTasksFile, TASKS_INDEX_SCHEMA_VERSION, TaskEntry,
    TaskIndexEntry, TaskMeta, TaskStatus, TasksIndexFile,
};
pub use progress::{
    ProgressUpdate, TaskResumeBrief, begin_existing_task, build_resume_brief, record_task_progress,
    render_task_progress_md, task_progress_rel_path, write_task_progress_md,
};

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
