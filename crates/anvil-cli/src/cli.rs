//! Top-level `clap` CLI definition for the `anvil` engineering harness.

pub mod subcmds;

use std::path::PathBuf;

use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Args, Parser, Subcommand};

use crate::ui::PLAIN_ANVIL_BANNER;
use subcmds::{
    ArchitectureSubcmd, CheckpointSubcmd, CircuitSubcmd, CodeSubcmd, ContextSubcmd, DaemonSubcmd,
    DocsSubcmd, GraphSubcmd, GuardSubcmd, MemSubcmd, OrchestratorSubcmd, PluginSubcmd,
    PrereqsSubcmd, QaSubcmd, RatchetSubcmd, ReviewSubcmd, SonarSubcmd, SpecSubcmd, TasksSubcmd,
    WorktreeSubcmd,
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

/// Arguments of `anvil search` (graph -> docs -> code lookup).
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Symbol, file, concept, or text to find (quote it to search the code for that exact text).
    pub query: String,
    /// Also search the code (grep) even when the graph and docs answered.
    #[arg(long)]
    pub code: bool,
    /// Most graph hits listed (docs list at most 5).
    #[arg(long, default_value_t = 8)]
    pub limit: usize,
    /// Token budget shared by the graph, docs, and code sections.
    #[arg(long, default_value_t = 1200)]
    pub max_tokens: usize,
    /// Print the full report as JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments of `anvil grep` (ripgrep-style flags, token-bounded output).
#[derive(Debug, Args)]
pub struct GrepArgs {
    /// Regex to search for (literal with `-F`).
    pub pattern: String,
    /// Repo-relative files or directories to search (default: the whole repository).
    pub paths: Vec<String>,
    /// Always ignore case.
    #[arg(short = 'i', long)]
    pub ignore_case: bool,
    /// Ignore case unless the pattern has an uppercase letter (already the default).
    #[arg(short = 'S', long)]
    pub smart_case: bool,
    /// Match case exactly (disables smart case).
    #[arg(short = 's', long)]
    pub case_sensitive: bool,
    /// Treat the pattern as a literal string.
    #[arg(short = 'F', long)]
    pub fixed_strings: bool,
    /// Match whole words only.
    #[arg(short = 'w', long = "word-regexp")]
    pub word: bool,
    /// Only search files matching this glob (repeatable; prefix `!` to exclude).
    #[arg(short = 'g', long = "glob")]
    pub globs: Vec<String>,
    /// Lines of context around every shown match.
    #[arg(short = 'C', long, default_value_t = 0)]
    pub context: usize,
    /// List matching files with match counts instead of lines.
    #[arg(short = 'l', long = "files-with-matches")]
    pub files_only: bool,
    /// Also search dot-files and dot-directories.
    #[arg(long)]
    pub hidden: bool,
    /// Maximum matches shown across all files.
    #[arg(long, default_value_t = 50)]
    pub max_matches: usize,
    /// Maximum matches shown per file.
    #[arg(long, default_value_t = 10)]
    pub max_per_file: usize,
    /// Approximate token budget of the output.
    #[arg(long, default_value_t = 1500)]
    pub max_tokens: usize,
    /// Print the full report as JSON.
    #[arg(long)]
    pub json: bool,
    /// Search engine: `auto`, `builtin`, or `tgrep` (default: `search.backend` from the config).
    #[arg(long)]
    pub backend: Option<String>,
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
        /// Architecture style to enforce (`auto` detects one and prompts interactively, or `none`, `hexagonal`, `clean`, `layered`, `feature-sliced`, `modular-monolith`, `event-driven`).
        #[arg(long, default_value = "auto", value_parser = ["auto", "none", "hexagonal", "clean", "layered", "feature-sliced", "modular-monolith", "event-driven"])]
        architecture: String,
    },
    /// Non-destructively sync skills, config tools, and adapter files.
    Sync {
        /// Target AI adapter (`auto`, `all`, `antigravity`, `claude`).
        #[arg(long, default_value = "auto")]
        adapter: String,
    },
    /// Audit harness health, strict configs, graph, memory, identity, and stack tools.
    Doctor {
        /// Run the full `anvil prereqs check` inside doctor (required failures fail doctor).
        #[arg(long)]
        prereqs: bool,
    },
    /// Execute an Antigravity or Claude Code lifecycle hook (`install-git` installs pre-commit hook).
    Hook {
        /// Hook event name (`session-start`, `user-prompt-submit`, `pre-tool-use`, `post-tool-use`, `pre-invocation`, `stop`, `install-git`).
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
        /// Check only changes inside these files or directories (an executor's `SCOPE:`).
        #[arg(long, value_delimiter = ',')]
        scope: Vec<String>,
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
    /// Inspect and configure the security guard for destructive commands (cloud, infra, SQL, git).
    Guard {
        #[command(subcommand)]
        sub: GuardSubcmd,
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
    /// Start here: look a query up in the code graph, then the docs, then the code (grep) only if needed.
    Search(SearchArgs),
    /// Fast gitignore-aware code search grouped by file, annotated with the enclosing symbol.
    Grep(GrepArgs),
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
    /// Record and list review findings that surfaced after `anvil check` was green.
    Review {
        #[command(subcommand)]
        sub: ReviewSubcmd,
    },
    /// Install the Claude Code mod (executor model and effort picker) for the user account.
    Plugin {
        #[command(subcommand)]
        sub: PluginSubcmd,
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
    /// Living project documentation (Open Knowledge Format) kept in sync with the code.
    Docs {
        #[command(subcommand)]
        sub: DocsSubcmd,
    },
    /// Detect and check what a machine needs for this project: runtimes, tools, env files, MCP servers.
    Prereqs {
        #[command(subcommand)]
        sub: PrereqsSubcmd,
    },
    /// Architecture style presets (hexagonal, clean, ...): record one, see its rules, check the repo.
    Architecture {
        #[command(subcommand)]
        sub: ArchitectureSubcmd,
    },
    /// Execute sandboxed JS Code Mode queries against `anvil-graph` and `anvil-memory`.
    Code {
        #[command(subcommand)]
        sub: CodeSubcmd,
    },
}
