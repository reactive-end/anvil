//! Command dispatcher and shared CLI repository resolution helpers.

pub mod check;
pub mod circuit;
pub mod context;
pub mod daemon;
pub mod deps;
pub mod diff;
pub mod doctor;
pub mod graph;
pub mod grep;
pub mod hook;
pub mod impact;
pub mod init;
pub mod mcp;
pub mod mem;
pub mod qa;
pub mod ratchet;
pub mod run;
pub mod stats;
pub mod sync;
pub mod tasks;

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use anvil_core::git;

use crate::cli::{Cli, Command};

/// Sentinel error carrying a specific process exit code without printing a noisy backtrace.
#[derive(Debug)]
pub struct ExitCodeError {
    pub code: i32,
    pub message: Option<String>,
}

impl ExitCodeError {
    pub fn silent(code: i32) -> Self {
        Self { code, message: None }
    }

    pub fn with_message(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: Some(message.into()),
        }
    }
}

impl fmt::Display for ExitCodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.message {
            Some(msg) => f.write_str(msg),
            None => write!(f, "exited with status {}", self.code),
        }
    }
}

impl std::error::Error for ExitCodeError {}

/// Dispatches parsed CLI arguments to the corresponding thin command module.
pub fn dispatch(cli: Cli) -> Result<()> {
    let root = resolve_repo_root(cli.root.as_deref())?;
    match cli.command {
        Command::Init {
            path,
            adapter,
            git_hooks,
        } => init::execute(
            &resolve_init_root(path.as_deref(), &root)?,
            &adapter,
            git_hooks,
        ),
        Command::Sync { adapter } => sync::execute(&root, &adapter),
        Command::Doctor => doctor::execute(&root),
        Command::Hook { event, adapter } => hook::execute(&root, &event, &adapter),
        Command::Run { cmd } => run::execute(&root, &cmd),
        Command::Check {
            full,
            tier,
            no_tests,
            no_cache,
            staged,
        } => check::execute(&root, full, tier, no_tests, no_cache, staged),
        Command::Impact {
            files,
            depth,
            origin,
            typecheck,
        } => impact::execute(&root, &files, depth, origin.as_deref(), typecheck),
        Command::Graph { sub } => graph::execute(&root, sub),
        Command::Mem { sub } => mem::execute(&root, sub),
        Command::Mcp => mcp::execute(&root),
        Command::Tasks { sub } => tasks::execute(&root, sub),
        Command::Ratchet { sub } => ratchet::execute(&root, sub),
        Command::Circuit { sub } => circuit::execute(&root, sub),
        Command::Deps { all, manifests } => deps::execute(&root, all, &manifests),
        Command::Qa { sub } => qa::execute(&root, sub),
        Command::Context { sub } => context::execute(&root, sub),
        Command::Diff {
            max_lines,
            max_tokens,
        } => diff::execute(&root, max_lines, max_tokens),
        Command::Grep {
            pattern,
            max_matches,
            max_tokens,
        } => grep::execute(&root, &pattern, max_matches, max_tokens),
        Command::Daemon { sub } => daemon::execute(&root, sub),
        Command::Stats { reset } => stats::execute(&root, reset),
    }
}

/// Resolves the repository root from `--root`, or Git toplevel, or current working directory.
pub fn resolve_repo_root(override_root: Option<&Path>) -> Result<PathBuf> {
    if let Some(dir) = override_root {
        return Ok(dir.to_path_buf());
    }
    let cwd = std::env::current_dir().context("failed to read current working directory")?;
    Ok(git::repo_root(&cwd).unwrap_or(cwd))
}

fn resolve_init_root(path: Option<&Path>, default_root: &Path) -> Result<PathBuf> {
    match path {
        Some(p) => Ok(p.to_path_buf()),
        None => Ok(default_root.to_path_buf()),
    }
}
