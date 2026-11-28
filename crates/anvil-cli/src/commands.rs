//! Command dispatcher and shared CLI repository resolution helpers.

pub mod architecture;
pub mod check;
pub mod checkpoint;
pub mod circuit;
pub mod code;
pub mod context;
pub mod daemon;
pub mod db;
pub mod deps;
pub mod diff;
pub mod docs;
pub mod doctor;
pub mod graph;
pub mod grep;
pub mod guard;
pub mod hook;
pub mod impact;
pub mod init;
pub mod mcp;
pub mod mem;
pub mod orchestrator;
pub mod plugin;
pub mod prereqs;
pub mod prompt;
pub mod qa;
pub mod ratchet;
pub mod readiness;
pub mod review;
pub mod run;
pub mod search;
pub mod sonar;
pub mod spec;
pub mod stats;
pub mod sync;
pub mod tasks;
pub mod worktree;

use std::fmt;
use std::path::{Path, PathBuf};

use anvil_core::git;
use anyhow::{Context, Result};

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
        Self { code, message: Some(message.into()) }
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
        Command::Init { path, adapter, git_hooks, sonar, layout, architecture } => init::execute(
            &resolve_init_root(path.as_deref(), &root)?,
            init::InitOptions {
                adapter: &adapter,
                git_hooks,
                sonar: &sonar,
                layout: &layout,
                architecture: &architecture,
            },
        ),
        Command::Sync { adapter } => sync::execute(&root, &adapter),
        Command::Doctor { prereqs } => doctor::execute(&root, prereqs),
        Command::Hook { event, adapter } => hook::execute(&root, &event, &adapter),
        Command::Run { cmd } => run::execute(&root, &cmd),
        Command::Check { full, tier, no_tests, no_cache, staged, sonar, no_sonar, scope } => {
            let opts = check::CheckOptions {
                full,
                tier_override: tier,
                no_tests,
                no_cache,
                staged,
                sonar,
                no_sonar,
            };
            check::execute(&root, opts, &scope)
        }
        Command::Impact { files, depth, origin, typecheck } => {
            impact::execute(&root, &files, depth, origin.as_deref(), typecheck)
        }
        Command::Graph { sub } => graph::execute(&root, sub),
        Command::Mem { sub } => mem::execute(&root, sub),
        Command::Mcp { proxy } => mcp::execute(&root, proxy),
        Command::Tasks { sub } => tasks::execute(&root, sub),
        Command::Ratchet { sub } => ratchet::execute(&root, sub),
        Command::Circuit { sub } => circuit::execute(&root, sub),
        Command::Guard { sub } => guard::execute(&root, sub),
        Command::Deps { all, manifests } => deps::execute(&root, all, &manifests),
        Command::Qa { sub } => qa::execute(&root, sub),
        Command::Db { sub } => db::execute(&root, sub),
        Command::Context { sub } => context::execute(&root, sub),
        Command::Diff { max_lines, max_tokens } => diff::execute(&root, max_lines, max_tokens),
        Command::Search(args) => search::execute(&root, &args),
        Command::Grep(args) => grep::execute(&root, &args),
        Command::Daemon { sub } => daemon::execute(&root, sub),
        Command::Stats { reset, trajectories } => stats::execute(&root, reset, trajectories),
        Command::Orchestrator { sub } => orchestrator::execute(&root, sub),
        Command::Plugin { sub } => plugin::execute(sub),
        Command::Review { sub } => review::execute(&root, sub),
        Command::Sonar { sub } => sonar::execute(&root, sub),
        Command::Checkpoint { sub } => checkpoint::execute(&root, sub),
        Command::Worktree { sub } => worktree::execute(&root, sub),
        Command::Spec { sub } => spec::execute(&root, sub),
        Command::Code { sub } => code::execute(&root, sub),
        Command::Docs { sub } => docs::execute(&root, sub),
        Command::Prereqs { sub } => prereqs::execute(&root, sub),
        Command::Architecture { sub } => architecture::execute(&root, sub),
        Command::Prompt { sub } => prompt::execute(&root, sub),
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
