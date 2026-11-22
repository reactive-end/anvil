//! Nested `clap` subcommand definitions for `anvil-cli`.

use std::path::PathBuf;

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum GraphSubcmd {
    /// Full repository code graph build.
    Build {
        /// Also export NetworkX JSON to `.harness/.state/graph.json`.
        #[arg(long)]
        json: bool,
        /// Optional path to export static HTML visualization.
        #[arg(long)]
        html: Option<PathBuf>,
        /// Optional `graphify-out/graph.json` semantic overlay path.
        #[arg(long)]
        overlay: Option<PathBuf>,
    },
    /// Incremental graph update via `blake3` hashes.
    Update,
    /// Read only the AST line span of a symbol (plus optional imports).
    ReadSymbol {
        symbol: String,
        /// Omit file-level import lines from output.
        #[arg(long)]
        no_imports: bool,
    },
    /// Print compact signature-only outline for a file, directory, or repository.
    Outline {
        /// Optional repo-relative file or directory prefix.
        target: Option<String>,
        #[arg(long, default_value_t = 1500)]
        max_tokens: usize,
    },
    /// BM25/token-scored symbol and file search.
    Query {
        term: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Shortest directed dependency path between two nodes.
    Path { from: String, to: String },
    /// Explain neighbors, community, and god-node status for a node.
    Explain { target: String },
    /// Generate compact Markdown architecture report.
    Report {
        #[arg(long)]
        html: Option<PathBuf>,
    },
    /// Merge multiple sub-repository graphs into a unified multi-repo graph.
    Merge {
        /// Sub-repository directories (defaults to configured `subrepos`).
        dirs: Vec<PathBuf>,
        /// Optional output path for merged binary graph.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum MemSubcmd {
    /// Save or upsert a persistent memory observation.
    Save {
        #[arg(long)]
        kind: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        what: String,
        #[arg(long)]
        why: String,
        #[arg(long = "where", visible_alias = "location")]
        location: String,
        #[arg(long)]
        learned: String,
        #[arg(long)]
        topic_key: Option<String>,
        #[arg(long, value_delimiter = ',')]
        files: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        symbols: Vec<String>,
        #[arg(long)]
        ttl: Option<i64>,
    },
    /// Progressive-disclosure BM25 FTS5 search.
    Search {
        query: String,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Chronological neighborhood around a memory ID or topic_key.
    Timeline {
        id_or_topic: String,
        #[arg(long, default_value_t = 5)]
        before: usize,
        #[arg(long, default_value_t = 5)]
        after: usize,
    },
    /// Fetch full observation details by ID or topic_key.
    Get { id: String },
    /// Load token-budgeted session startup memory context.
    Context {
        #[arg(long, default_value_t = 800)]
        max_tokens: usize,
    },
    /// Sync Git chunks from `.harness/memory/chunks/` into SQLite.
    Sync,
    /// Cross-check memory observations against `anvil-graph` freshness.
    CheckStale,
    /// Remove expired `session` observations.
    Cleanup,
}

#[derive(Debug, Subcommand)]
pub enum TasksSubcmd {
    /// Start a new tracked task in `.harness/tasks/`.
    Start {
        #[arg(required = true)]
        request: Vec<String>,
        #[arg(long)]
        author: Option<String>,
    },
    /// Append a progress entry to an existing task.
    Update {
        task_id: String,
        summary: String,
        #[arg(long, default_value = "in_progress")]
        status: String,
        #[arg(long, value_delimiter = ',')]
        files: Vec<String>,
        #[arg(long)]
        author: Option<String>,
    },
    /// Mark a task as done with a final summary entry.
    Done {
        task_id: String,
        summary: String,
        #[arg(long, value_delimiter = ',')]
        files: Vec<String>,
        #[arg(long)]
        author: Option<String>,
    },
    /// Write an isolated per-author `plan-<user>.md` for a task.
    Plan {
        task_id: String,
        /// Markdown text or path to a `.md` file.
        markdown_or_file: String,
        #[arg(long)]
        author: Option<String>,
    },
    /// Deterministically rebuild root `tasks.json`.
    Build,
    /// List all tasks in `.harness/tasks/`.
    List,
    /// Git custom merge driver for conflict-free `tasks.json` resolution.
    MergeDriver {
        base: PathBuf,
        current: PathBuf,
        other: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum RatchetSubcmd {
    /// Log a recurring mistake candidate to `.harness/candidates/`.
    Log {
        #[arg(long)]
        rule: String,
        #[arg(long)]
        description: String,
        #[arg(long)]
        pattern: String,
        #[arg(long, value_delimiter = ',')]
        lang: Vec<String>,
        #[arg(long)]
        high_impact: bool,
        #[arg(long, default_value_t = 10)]
        runtime_ms: u64,
        #[arg(long)]
        example: Option<String>,
    },
    /// List pending candidates and promoted rules.
    List,
    /// Evaluate candidate against the triple ratchet filter.
    Review {
        id: String,
        /// Optional clean code sample strings or file paths to scan for false positives.
        #[arg(long)]
        samples: Vec<String>,
    },
    /// Test a candidate regex across matching repository files and measure runtime.
    Test { id: String },
    /// Promote an eligible candidate into `.harness/rules/`.
    Promote { id: String },
}

#[derive(Debug, Subcommand)]
pub enum CircuitSubcmd {
    /// Show current Circuit Breaker state.
    Status,
    /// Reset an OPEN Circuit Breaker back to CLOSED.
    Reset,
}

#[derive(Debug, Subcommand)]
pub enum QaSubcmd {
    /// Detect UI platform and display onboarding driver options.
    Onboard,
    /// Persist selected web QA driver (`chrome_dev_tools`, `playwright`, `browser`, `skip`).
    SelectWeb { driver: String },
    /// Persist selected mobile QA driver (`adb_mobile_mcp`, `roborazzi`, `maestro`, `skip`).
    SelectMobile { driver: String },
    /// Evaluate raw `dom_audit.js` JSON into a compressed QA report.
    EvalWeb {
        #[arg(long)]
        dom_json: PathBuf,
        #[arg(long, default_value = "desktop")]
        viewport: String,
        #[arg(long, default_value_t = 1440)]
        width: u32,
        #[arg(long, default_value_t = 900)]
        height: u32,
        #[arg(long, value_delimiter = ',')]
        console_errors: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        page_errors: Vec<String>,
    },
    /// Run headless Playwright DOM + screenshot capture on a URL.
    Run {
        url: String,
        /// Screenshot capture phase (`before` or `after`).
        #[arg(long, default_value = "after", value_parser = ["before", "after"])]
        phase: String,
        #[arg(long, default_value = ".harness/.state/qa")]
        out_dir: PathBuf,
    },
    /// Scan Android `uiautomator` XML, `logcat`, and Roborazzi diffs.
    ScanMobile {
        #[arg(long)]
        xml: Option<PathBuf>,
        #[arg(long)]
        logcat: Option<PathBuf>,
        #[arg(long, default_value_t = 1080)]
        width: i32,
        #[arg(long, default_value_t = 2400)]
        height: i32,
    },
}

#[derive(Debug, Subcommand)]
pub enum ContextSubcmd {
    /// Build a compact minimal context packet from `anvil-graph` and `anvil-memory`.
    Pack {
        #[arg(required = true)]
        targets: Vec<String>,
        #[arg(long, default_value_t = 1200)]
        max_tokens: usize,
    },
}

#[derive(Debug, Subcommand)]
pub enum DaemonSubcmd {
    /// Start the resident local `anvil-graph` daemon.
    Start {
        /// Run in the background (default).
        #[arg(long)]
        background: bool,
        /// Internal blocking foreground server mode.
        #[arg(long, hide = true)]
        foreground: bool,
    },
    /// Stop the running daemon for this repository.
    Stop,
    /// Show current daemon status, PID, port, and loaded graph counts.
    Status,
}
