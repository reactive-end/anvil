//! Runs a [`PrereqSet`] against a [`Host`] and collects a [`Report`].

mod custom;
mod files;
mod mcp;
mod secrets;
mod tools;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anvil_core::config::prereqs::PrereqSet;

use crate::host::Host;
use crate::project::Project;
use crate::report::{Item, Report, Status};

/// Time limits of a check.
#[derive(Debug, Clone, Copy)]
pub struct CheckOptions {
    /// Per version probe.
    pub tool_timeout: Duration,
    /// Per custom check command.
    pub custom_timeout: Duration,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self { tool_timeout: Duration::from_secs(5), custom_timeout: Duration::from_secs(30) }
    }
}

/// Where a check runs: the workspace root and its projects.
pub(crate) struct Ctx<'a> {
    pub root: &'a Path,
    pub projects: &'a [Project],
    pub opts: CheckOptions,
}

impl Ctx<'_> {
    /// Directory of the subrepo `name`, or the root when `None` or unknown.
    pub(crate) fn dir_of(&self, subrepo: Option<&str>) -> PathBuf {
        self.projects
            .iter()
            .find(|p| p.name.as_deref() == subrepo)
            .map_or_else(|| self.root.to_path_buf(), |p| p.dir(self.root))
    }
}

/// Checks every requirement of `set`; never writes anything and never prints a secret value.
pub fn check(
    host: &dyn Host,
    root: &Path,
    projects: &[Project],
    set: &PrereqSet,
    opts: &CheckOptions,
) -> Report {
    let ctx = Ctx { root, projects, opts: *opts };
    let mut items = tools::check_tools(host, &ctx, &set.tools);
    items.extend(files::check_env_files(&ctx, &set.env_files));
    items.extend(files::check_env_vars(host, &ctx, &set.env_vars));
    items.extend(files::check_paths(&ctx, &set.paths));
    items.extend(mcp::check_mcp(host, &ctx, &set.mcp_servers));
    items.extend(custom::check_custom(host, &ctx, &set.custom));
    items.extend(secrets::scan(&ctx));
    let rank =
        |item: &Item| projects.iter().position(|p| p.name == item.subrepo).unwrap_or(usize::MAX);
    items.sort_by_key(rank);
    Report::new(items)
}

/// A failed check is `bad` when the requirement is required and only a warning otherwise.
pub(crate) fn failure_status(required: bool, bad: Status) -> Status {
    if required { bad } else { Status::Warning }
}
