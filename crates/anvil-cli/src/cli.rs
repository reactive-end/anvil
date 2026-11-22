//! Top-level `clap` CLI definition for the `anvil` engineering harness.

pub mod subcmds;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use subcmds::{
    CircuitSubcmd, ContextSubcmd, DaemonSubcmd, GraphSubcmd, MemSubcmd, QaSubcmd, RatchetSubcmd,
    TasksSubcmd,
};

/// `anvil` — Native AI Engineering Harness CLI.
#[derive(Debug, Parser)]
#[command(name = "anvil", version, about = "Anvil AI engineering harness")]
pub struct Cli {
    /// Override repository root directory.
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize `.harness/` and thin adapters in a repository.
    Init {
        /// Target repository directory (defaults to current directory).
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Non-destructively sync `.harness/skills/` and adapter configs.
    Sync,
    /// Audit harness health, strict configs, graph, memory, identity, and stack tools.
    Doctor,
    /// Execute an Antigravity or Claude Code lifecycle hook reading JSON from stdin.
    Hook {
        /// Hook event name (`session-start`, `pre-tool-use`, `post-tool-use`, `pre-invocation`, `stop`).
        event: String,
        /// Target adapter (`antigravity` or `claude`).
        #[arg(long)]
        adapter: String,
    },
    /// Run a command with token-compressed output and Circuit Breaker tracking.
    Run {
        /// Command and arguments to execute.
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },
    /// Classify git diff and run Tier 0 / Tier 1 / Tier 2 verification.
    Check {
        /// Run full Tier 2 verification (including deps & visual QA checks).
        #[arg(long)]
        full: bool,
        /// Force a specific tier (`0`, `1`, or `2`).
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=2))]
        tier: Option<u8>,
        /// Skip Tier 1 targeted test execution.
        #[arg(long)]
        no_tests: bool,
        /// Bypass the Tier 1 content-addressed check cache.
        #[arg(long)]
        no_cache: bool,
    },
    /// Analyze reverse-BFS blast radius and upstream origin via `anvil-graph`.
    Impact {
        /// Modified files (defaults to git working-tree changes).
        files: Vec<String>,
        /// Maximum reverse-BFS traversal depth.
        #[arg(long, default_value_t = 2)]
        depth: usize,
        /// Trace upstream source-of-truth for a symbol or file.
        #[arg(long)]
        origin: Option<String>,
        /// Run native typecheckers filtered to changed + impacted files.
        #[arg(long)]
        typecheck: bool,
    },
    /// Build, update, query, inspect, and merge `anvil-graph`.
    Graph {
        #[command(subcommand)]
        sub: GraphSubcmd,
    },
    /// Save, search, timeline, inspect, sync, and validate `anvil-memory`.
    Mem {
        #[command(subcommand)]
        sub: MemSubcmd,
    },
    /// Start the unified stdio MCP server (`graph_*` and `mem_*`).
    Mcp,
    /// Manage conflict-free team task ledger and root `tasks.json`.
    Tasks {
        #[command(subcommand)]
        sub: TasksSubcmd,
    },
    /// Log, list, review, and promote governed linter ratchet rules.
    Ratchet {
        #[command(subcommand)]
        sub: RatchetSubcmd,
    },
    /// Inspect or reset the N=3 Circuit Breaker.
    Circuit {
        #[command(subcommand)]
        sub: CircuitSubcmd,
    },
    /// Audit dependencies for deprecation or staleness across npm/PyPI/pub.dev/Maven/Cargo.
    Deps {
        /// Scan all supported manifests in the repository instead of git diff.
        #[arg(long)]
        all: bool,
        /// Explicit manifest file paths to audit.
        manifests: Vec<PathBuf>,
    },
    /// Run hybrid Web & Mobile visual QA onboarding, DOM evaluation, and view-tree scans.
    Qa {
        #[command(subcommand)]
        sub: QaSubcmd,
    },
    /// Build compact minimal context packets for subagent delegation.
    Context {
        #[command(subcommand)]
        sub: ContextSubcmd,
    },
    /// Compress `git diff HEAD`, drop noisy files, and annotate affected symbols.
    Diff {
        #[arg(long, default_value_t = 200)]
        max_lines: usize,
        #[arg(long, default_value_t = 2000)]
        max_tokens: usize,
    },
    /// Noise-filtered, binary-safe, token-bounded repository grep.
    Grep {
        pattern: String,
        #[arg(long, default_value_t = 50)]
        max_matches: usize,
        #[arg(long, default_value_t = 1500)]
        max_tokens: usize,
    },
    /// Manage the resident local `anvil-graph` TCP daemon.
    Daemon {
        #[command(subcommand)]
        sub: DaemonSubcmd,
    },
    /// View or reset local telemetry, cache hits, and token-savings stats.
    Stats {
        /// Reset `.harness/.state/stats.json` to zeroed defaults.
        #[arg(long)]
        reset: bool,
    },
}
