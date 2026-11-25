//! Top-level `clap` CLI definition for the `anvil` engineering harness.

pub mod subcmds;

use std::path::PathBuf;

use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Parser, Subcommand};

use crate::ui::PLAIN_ANVIL_BANNER;
use subcmds::{
    CheckpointSubcmd, CircuitSubcmd, CodeSubcmd, ContextSubcmd, DaemonSubcmd, GraphSubcmd,
    MemSubcmd, OrchestratorSubcmd, QaSubcmd, RatchetSubcmd, SonarSubcmd, SpecSubcmd, TasksSubcmd, WorktreeSubcmd,
};

fn clap_styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Yellow.on_default() | Effects::BOLD)
        .usage(AnsiColor::Yellow.on_default() | Effects::BOLD)
        .literal(AnsiColor::Cyan.on_default() | Effects::BOLD)
        .placeholder(AnsiColor::BrightBlack.on_default())
        .valid(AnsiColor::Green.on_default() | Effects::BOLD)
        .invalid(AnsiColor::Red.on_default() | Effects::BOLD)
        .error(AnsiColor::Red.on_default() | Effects::BOLD)
}

/// `anvil` — Native AI Engineering Harness CLI.
#[derive(Debug, Parser)]
#[command(
    name = "anvil",
    version,
    about = "Native AI Engineering Harness",
    before_help = PLAIN_ANVIL_BANNER,
    styles = clap_styles()
)]
pub struct Cli {
    /// Override repository root directory.
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,

    /// Disable ANSI colors and decorative terminal output.
    #[arg(long, global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize `.harness/` and thin adapters in a repository (and independent subprojects).
    Init {
        /// Target repository directory (defaults to current directory).
        #[arg(long)]
        path: Option<PathBuf>,
        /// Target AI adapter (`auto`, `all`, `antigravity`, `claude`).
        #[arg(long, default_value = "auto")]
        adapter: String,
        /// Install a non-destructive `.git/hooks/pre-commit` runner (`anvil check --staged --tier 0`).
        #[arg(long)]
        git_hooks: bool,
        /// Enable opt-in local SonarQube integration (`auto` prompts interactively, or `yes` / `no`).
        #[arg(long, default_value = "auto", value_parser = ["auto", "yes", "no"])]
        sonar: String,
        /// Workspace layout when multiple projects exist (`auto` prompts interactively, or `monorepo` / `multi-repo`).
        #[arg(long, default_value = "auto", value_parser = ["auto", "monorepo", "multi-repo"])]
        layout: String,
    },
    /// Non-destructively sync skills, config tools, and adapter files.
    Sync {
        /// Target AI adapter (`auto`, `all`, `antigravity`, `claude`).
        #[arg(long, default_value = "auto")]
        adapter: String,
    },
    /// Audit harness health, strict configs, graph, memory, identity, and stack tools.
    Doctor,
    /// Execute an Antigravity or Claude Code lifecycle hook (`install-git` installs pre-commit hook).
    Hook {
        /// Hook event name (`session-start`, `pre-tool-use`, `post-tool-use`, `pre-invocation`, `stop`, `install-git`).
        event: String,
        /// Target adapter (`antigravity` or `claude`).
        #[arg(long, default_value = "antigravity")]
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
        /// Verify only git staged files (`git diff --cached --name-only`).
        #[arg(long)]
        staged: bool,
        /// Explicitly run opt-in SonarQube checks (complexity, duplication, quality gate).
        #[arg(long)]
        sonar: bool,
        /// Skip SonarQube checks even when `sonar.enabled` is true in config.
        #[arg(long)]
        no_sonar: bool,
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
    /// Start the unified stdio MCP server (`graph_*`, `mem_*`, `code_exec`) or `--proxy` gateway.
    Mcp {
        /// Run as a Code Mode proxy gateway wrapping `.harness/mcp-proxy.json` upstream servers.
        #[arg(long)]
        proxy: bool,
    },
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
    /// Audit dependencies for deprecation or staleness across npm/PyPI/pub.dev/Maven/Cargo/Composer.
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
        /// Show hook latency percentiles (p50/p95), Code Mode savings, and trajectory summary.
        #[arg(long)]
        trajectories: bool,
    },
    /// Pair a planning model with an execution model for Claude Code (model + effort per role).
    Orchestrator {
        #[command(subcommand)]
        sub: OrchestratorSubcmd,
    },
    /// Opt-in local SonarQube Quality Gate, New Code issues, and scanner runner.
    Sonar {
        #[command(subcommand)]
        sub: SonarSubcmd,
    },
    /// Save, list, or rewind isolated Git working-tree checkpoints.
    Checkpoint {
        #[command(subcommand)]
        sub: CheckpointSubcmd,
    },
    /// Create, list, or remove isolated Git worktrees under `.harness/.worktrees/`.
    Worktree {
        #[command(subcommand)]
        sub: WorktreeSubcmd,
    },
    /// Scaffold and verify `.harness/specs/*.md` deterministic acceptance criteria.
    Spec {
        #[command(subcommand)]
        sub: SpecSubcmd,
    },
    /// Execute sandboxed JS Code Mode queries against `anvil-graph` and `anvil-memory`.
    Code {
        #[command(subcommand)]
        sub: CodeSubcmd,
    },
}
