//! Repeatable browser flows: scripted steps with network, text, and visual expectations.
//!
//! A flow (`.harness/qa/flows/<name>.json`) is parsed by [`spec`], turned into a Playwright script
//! by [`plan`], observed in a real browser by [`run`], judged by [`eval`], and rendered by
//! [`summary`]. Screenshots are compared with stored baselines by [`diff`]; [`server`] starts and
//! stops the app under test.

pub mod diff;
pub mod eval;
pub mod plan;
pub mod report;
pub mod run;
pub mod server;
pub mod spec;
pub mod summary;

use std::path::{Path, PathBuf};

use anvil_core::config::qa::WebQaConfig;

/// Directory holding the flow files of this repository.
pub fn flows_dir(root: &Path, web: &WebQaConfig) -> PathBuf {
    root.join(&web.flows_dir)
}
