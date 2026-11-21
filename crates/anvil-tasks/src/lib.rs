//! Conflict-free team task ledger and consolidated `tasks.json` builder.

pub mod builder;
pub mod identity;
pub mod ledger;
pub mod merge_driver;
pub mod model;

mod fs_util;

pub use builder::build_root_tasks_json;
pub use identity::resolve_author;
pub use ledger::{append_entry, append_entry_as, list_tasks, start_task, start_task_as, write_task_plan};
pub use merge_driver::{install_git_merge_driver, resolve_merge};
pub use model::{ConsolidatedTask, ConsolidatedTasksFile, TaskEntry, TaskMeta, TaskStatus};
