//! API scenarios: HTTP calls to the running backend followed by database checks.
//!
//! A scenario (`.harness/qa/api/<name>.json`) is parsed by [`spec`], its `{{variables}}` filled by
//! [`interpolate`], its calls made by [`http`] and [`db`] (Google MCP Toolbox for Databases), its
//! expectations judged by [`eval`], and the whole run recorded by [`run`] and rendered by
//! [`summary`]. [`crate::flows::server`] starts and stops the API under test.

pub mod db;
pub mod eval;
pub mod exec;
pub mod http;
pub mod interpolate;
pub mod report;
pub mod run;
pub mod spec;
pub mod summary;
#[cfg(test)]
mod testkit;

use std::path::{Path, PathBuf};

use anvil_core::config::qa::ApiQaConfig;

/// Directory holding the scenario files of this repository.
pub fn scenarios_dir(root: &Path, api: &ApiQaConfig) -> PathBuf {
    root.join(&api.scenarios_dir)
}
