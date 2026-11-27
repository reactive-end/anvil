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
    /// Start a new tracked task in `.harness/tasks/` with a clean curated engineering title.
    Start {
        #[arg(required = true)]
        request: Vec<String>,
        /// Optional subdirectory group (e.g. `voz`), creating `.harness/tasks/voz/T-001`.
        #[arg(long)]
        group: Option<String>,
        /// Optional short custom ID (e.g. `F039`); defaults to `T-001`, `T-002`, ...
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        author: Option<String>,
    },
    /// Create and record an LLM-curated task in one step (`done` by default).
    Record {
        /// Concise 3-18 word engineering title (never raw user chat).
        title: String,
        /// Concise 1-2 sentence technical summary.
        summary: String,
        #[arg(long, default_value = "done")]
        status: String,
        /// Optional subdirectory group (e.g. `voz`), creating `.harness/tasks/voz/T-001`.
        #[arg(long)]
        group: Option<String>,
        /// Optional short custom ID (e.g. `F039`); defaults to `T-001`, `T-002`, ...
        #[arg(long)]
        id: Option<String>,
        #[arg(long, value_delimiter = ',')]
        files: Vec<String>,
        #[arg(long)]
        author: Option<String>,
    },
    /// Explicitly skip task creation for minor tweaks or non-task edits (`0` entries in `tasks.json`).
    Skip {
        #[arg(required = true)]
        reason: Vec<String>,
    },
    /// Append a progress checkpoint entry to an existing task (updates `progress.md` and `tasks.json`).
    Update {
        task_id: String,
        summary: String,
        #[arg(long, default_value = "in_progress")]
        status: String,
        /// Completed milestones so far (comma-separated).
        #[arg(long, value_delimiter = ',')]
        done: Vec<String>,
        /// Remaining next steps for seamless recovery after interruption (comma-separated).
        #[arg(long, value_delimiter = ',')]
        next: Vec<String>,
        /// Optional blocker preventing completion.
        #[arg(long)]
        blocker: Option<String>,
        /// Optional test/check verification note.
        #[arg(long)]
        evidence: Option<String>,
        #[arg(long, value_delimiter = ',')]
        files: Vec<String>,
        #[arg(long)]
        author: Option<String>,
    },
    /// Mark a task as done with a final verified summary entry.
    Done {
        task_id: String,
        summary: String,
        /// Optional test/verification evidence attached to completion.
        #[arg(long)]
        evidence: Option<String>,
        #[arg(long, value_delimiter = ',')]
        files: Vec<String>,
        #[arg(long)]
        author: Option<String>,
    },
    /// Print the compact crash-recovery brief for an interrupted/in-progress task (or explicit ID).
    Resume {
        /// Optional task ID (`T-001` or `voz/F001`); defaults to active `in_progress` task.
        task_id: Option<String>,
    },
    /// Write an isolated per-author `plan-<user>.md` for a task.
    Plan {
        task_id: String,
        /// Markdown text or path to a `.md` file.
        markdown_or_file: String,
        #[arg(long)]
        author: Option<String>,
    },
    /// Import a `feature_list.json` / tasks JSON into conflict-free `.harness/tasks/` folders.
    Import {
        /// Optional path to the JSON file; omit to open the native OS / TUI file browser.
        file: Option<PathBuf>,
        /// Store imported tasks under `.harness/tasks/<group>/` (asks interactively if omitted).
        #[arg(long, conflicts_with = "flat")]
        group: Option<String>,
        /// Store imported tasks directly in `.harness/tasks/` without a subdirectory prompt.
        #[arg(long, conflicts_with = "group")]
        flat: bool,
        /// Use the in-terminal arrow-key TUI file browser instead of the native OS file dialog.
        #[arg(long)]
        tui: bool,
        /// Overwrite existing task metadata and append imported status entries for existing IDs.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        author: Option<String>,
    },
    /// Export `.harness/tasks/` back to a single-file `feature_list.json` snapshot.
    Export {
        /// Optional destination JSON file path (prints to stdout when omitted).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Filter exported tasks by subdirectory group name (e.g. `voz`).
        #[arg(long)]
        group: Option<String>,
        #[arg(long)]
        project: Option<String>,
    },
    /// Execute a task/feature's `verify` command and optionally record the result as evidence.
    Verify {
        task_id: String,
        /// Append the compressed result as an immutable evidence entry in `.harness/tasks/<id>/`.
        #[arg(long)]
        record: bool,
    },
    /// Rename legacy `T-YYYYMMDD-<ULID>` folders to short sequential codes (`T-001`, `T-002`, ...).
    MigrateIds,
    /// Deterministically rebuild root `tasks.json`.
    Build,
    /// List tasks in `.harness/tasks/` (optionally filtered by group or status).
    List {
        #[arg(long)]
        group: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Git custom merge driver for conflict-free `tasks.json` resolution.
    MergeDriver { base: PathBuf, current: PathBuf, other: PathBuf },
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
        #[arg(long, value_delimiter = ',')]
        glob: Vec<String>,
        #[arg(long)]
        severity: Option<String>,
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
    Promote {
        id: String,
        #[arg(long)]
        severity: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum CircuitSubcmd {
    /// Show current Circuit Breaker state.
    Status,
    /// Reset an OPEN Circuit Breaker back to CLOSED.
    Reset,
}

#[derive(Debug, Subcommand)]
pub enum GuardSubcmd {
    /// Show the decision per category and the custom patterns (invalid regexes are marked).
    List,
    /// Set the decision of a category (`git`, `filesystem`, `disk`, `database`, `cloud`, `infra`,
    /// `containers`, `secrets`, `opaque`) to `ask`, `deny`, or `allow`.
    Set { category: String, decision: String },
    /// Add a regex that resolves matching commands to `ask`, `deny`, or `allow`.
    Add {
        decision: String,
        #[arg(allow_hyphen_values = true)]
        pattern: String,
    },
    /// Remove a regex from every pattern list.
    Remove {
        #[arg(allow_hyphen_values = true)]
        pattern: String,
    },
    /// Restore the default decisions and drop every custom pattern.
    Reset,
    /// Print the decision, category, and reason for a command without running it.
    Test {
        #[arg(allow_hyphen_values = true)]
        command: String,
        /// Tool name the command would arrive from (`Bash`, `mcp__aws__call_aws`, `Read`, ...).
        #[arg(long)]
        tool: Option<String>,
    },
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

#[derive(Debug, Subcommand)]
pub enum OrchestratorSubcmd {
    /// Choose both models and efforts (interactive without flags) and generate the agents.
    Setup {
        /// Orchestrator model: `fable`, `opus`, `sonnet`, `haiku`, `inherit`, or a full `claude-*` ID.
        #[arg(long)]
        model: Option<String>,
        /// Orchestrator effort: `low`, `medium`, `high`, `xhigh`, or `max`.
        #[arg(long)]
        effort: Option<String>,
        /// Executor model (same values as `--model`).
        #[arg(long)]
        executor_model: Option<String>,
        /// Executor effort (same values as `--effort`).
        #[arg(long)]
        executor_effort: Option<String>,
    },
    /// Show the configured pair and whether the mode is on.
    Status {
        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Enforce the roles: the orchestrator only judges and executors write within scope.
    On {
        /// Also start new Claude Code sessions in this repository as the orchestrator agent.
        #[arg(long)]
        default_agent: bool,
    },
    /// Stop enforcing the roles and stop starting sessions as the orchestrator.
    Off,
    /// Choose which MCP servers executors load (a checklist without flags).
    Tools {
        /// Servers to add: a name such as `postgres`, or a full `mcp__<server>__*` entry.
        #[arg(long, value_delimiter = ',')]
        add: Vec<String>,
        /// Servers to remove (same forms as `--add`).
        #[arg(long, value_delimiter = ',')]
        remove: Vec<String>,
        /// Only list the configured servers and which ones executors load.
        #[arg(long)]
        list: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ReviewSubcmd {
    /// Record a real problem found in review of work that had already passed `anvil check`.
    Log {
        /// What was wrong, in one sentence.
        description: String,
        /// Kebab-case category slug, for example `error-handling` or `dead-code`.
        #[arg(long)]
        category: String,
        /// How serious the problem is.
        #[arg(long, default_value = "medium", value_parser = ["high", "medium", "low"])]
        severity: String,
        /// Where it is, as `path` or `path:line`.
        #[arg(long)]
        file: Option<String>,
        /// Who found it.
        #[arg(long, default_value = "orchestrator", value_parser = ["orchestrator", "human", "ci", "tool"])]
        source: String,
    },
    /// List recorded findings, oldest first.
    List {
        /// Only findings from the last N days.
        #[arg(long)]
        since: Option<u32>,
    },
    /// Print a compact review of a task's change and remember it for approval.
    Packet {
        /// Task to review; defaults to the active task.
        task_id: Option<String>,
    },
    /// Approve a task's reviewed change; refused if its files changed after the packet.
    Approve {
        /// Task whose packet was reviewed.
        task_id: String,
        /// What you verified, in one sentence.
        summary: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum PluginSubcmd {
    /// Install or upgrade the Claude Code mod (`anvil-mode`) for the whole user account.
    Install,
    /// Uninstall the Claude Code mod and remove the files Anvil wrote for it.
    Uninstall,
    /// Show the embedded and installed versions and where the marketplace points.
    Status {
        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum SonarSubcmd {
    /// Fetch opt-in SonarQube Quality Gate status via Web API.
    Status {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        branch: Option<String>,
    },
    /// List unresolved New Code issues from SonarQube Web API.
    Issues {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },
    /// Run local `sonar` / `sonar-scanner` through Anvil's output compressor.
    Analyze {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum CheckpointSubcmd {
    /// Save an isolated working-tree snapshot into `refs/anvil/checkpoints/*`.
    Save {
        #[arg(long)]
        label: Option<String>,
    },
    /// List saved Anvil checkpoints.
    List,
    /// Restore working tree files from the latest or specified checkpoint (with safety backup).
    Rewind { target: Option<String> },
    /// Delete a specific checkpoint (`all` removes every checkpoint ref).
    Drop { target: String },
}

#[derive(Debug, Subcommand)]
pub enum WorktreeSubcmd {
    /// Create an isolated Git worktree inside `.harness/.worktrees/<name>`.
    Add {
        name: String,
        #[arg(long)]
        branch: Option<String>,
    },
    /// List active Git worktrees.
    List,
    /// Remove an isolated worktree created under `.harness/.worktrees/`.
    Remove {
        name: String,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum SpecSubcmd {
    /// Scaffold a deterministic verification spec under `.harness/specs/<name>.md`.
    Init { name: String },
    /// List all verification specs in `.harness/specs/`.
    List,
    /// Verify `.harness/specs/*.md` acceptance criteria against the repository.
    Check { name: Option<String> },
}

#[derive(Debug, Subcommand)]
pub enum CodeSubcmd {
    /// Execute a sandboxed Code Mode JS script (`script` or `@path/to/script.js`).
    Exec {
        script: String,
        #[arg(long, default_value_t = 32)]
        max_calls: usize,
    },
}

#[derive(Debug, Subcommand)]
pub enum DocsSubcmd {
    /// Create the OKF bundle (`index.md`, `log.md`) of each project under `.harness/docs/`; idempotent.
    Init {
        /// Project (bundle) name; defaults to `docs.project`, the repository folder, or every subrepo.
        #[arg(long)]
        project: Option<String>,
    },
    /// Create a module concept `<module>/overview.md` with the standard sections.
    New {
        /// Module path inside the bundle, e.g. `orders` or `core/config`.
        module: String,
        /// Repo-relative globs of the code the concept documents (`x-anvil-paths`).
        #[arg(long, num_args = 1.., required = true)]
        paths: Vec<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Create a concept for every code module that has none yet, using the code graph.
    Scaffold {
        #[arg(long)]
        project: Option<String>,
        /// Levels below the project root that count as a module (containers such as `crates/` and
        /// source roots such as `src/` are not levels). Default: the folders under each source root.
        #[arg(long)]
        depth: Option<usize>,
    },
    /// List concepts with status, trust tier, and drift.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Validate every bundle; stale concepts are warnings (errors with `--strict`).
    Check {
        /// Treat stale concepts as failures.
        #[arg(long)]
        strict: bool,
        #[arg(long)]
        json: bool,
    },
    /// After updating a doc: regenerate the outline block, restamp `generated` and the synced commit, log it.
    Refresh {
        /// Concept ids (`<project>/<module>`).
        ids: Vec<String>,
        /// Refresh every stale concept.
        #[arg(long, conflicts_with = "ids")]
        stale: bool,
    },
    /// Record a human review of a concept (only a person runs this).
    Verify {
        id: String,
        /// Reviewer name (default: `git config user.name`).
        #[arg(long)]
        by: Option<String>,
    },
    /// Search the concepts (BM25 over title, description, tags, headings, and body).
    Search {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum PrereqsSubcmd {
    /// Print what the project files say a machine needs; `--write` stores it in `.harness/config.json`.
    Detect {
        /// Store the detected set under `prerequisites.detected` (your overrides are kept).
        #[arg(long)]
        write: bool,
    },
    /// Check this machine; exits 1 when a required tool is missing or at the wrong version.
    Check {
        /// Print the full report as JSON.
        #[arg(long)]
        json: bool,
        /// Check only this subrepo (plus the workspace-level tools).
        #[arg(long)]
        subrepo: Option<String>,
    },
    /// Write `init.sh` and `init.ps1`, thin wrappers around `anvil prereqs check`.
    Script {
        /// Directory to write them to (default: the repository root).
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ArchitectureSubcmd {
    /// Print the configured style per repo/subrepo, the rules generated from it, and the agent guidance.
    Show {
        /// Print the full report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Record the architecture style (`none`, `hexagonal`, `clean`, `layered`, `feature-sliced`, `modular-monolith`, `event-driven`).
    Set {
        /// Style to record.
        style: String,
        /// Record it for this subrepo instead of the repository root.
        #[arg(long)]
        subrepo: Option<String>,
        /// Directory the rules are anchored to, relative to the repo or subrepo (default `src`).
        #[arg(long)]
        source_root: Option<String>,
    },
    /// Suggest a style from the folder names under each source root (changes nothing).
    Detect,
    /// Check the whole repository against the generated rules; exits 1 on any violation.
    Check {
        /// Print the full report as JSON.
        #[arg(long)]
        json: bool,
    },
}
