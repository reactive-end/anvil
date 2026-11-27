//! Schema and persistence for `.harness/config.json`.

pub mod docs;
pub mod orchestrator;
pub mod search;
pub mod security_guard;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use docs::DocsConfig;
use orchestrator::OrchestratorConfig;
use search::SearchConfig;
use security_guard::SecurityGuardConfig;

/// Target AI coding assistants whose adapter files are synchronized by `anvil init` / `anvil sync`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterTarget {
    #[default]
    All,
    Antigravity,
    Claude,
    Cursor,
    Windsurf,
    Codex,
}

impl AdapterTarget {
    /// Parses CLI `--adapter` values (`auto` returns `Ok(None)`).
    pub fn parse_cli(raw: &str) -> Result<Option<Self>> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Ok(None),
            "all" | "both" => Ok(Some(Self::All)),
            "antigravity" | "agy" => Ok(Some(Self::Antigravity)),
            "claude" | "claude_code" | "claude-code" => Ok(Some(Self::Claude)),
            "cursor" => Ok(Some(Self::Cursor)),
            "windsurf" => Ok(Some(Self::Windsurf)),
            "codex" => Ok(Some(Self::Codex)),
            other => bail!(
                "unsupported adapter target `{other}` (expected auto|all|antigravity|claude|cursor|windsurf|codex)"
            ),
        }
    }

    pub fn includes_antigravity(self) -> bool {
        matches!(self, Self::All | Self::Antigravity)
    }

    pub fn includes_claude(self) -> bool {
        matches!(self, Self::All | Self::Claude)
    }

    pub fn includes_cursor(self) -> bool {
        matches!(self, Self::Cursor)
    }

    pub fn includes_windsurf(self) -> bool {
        matches!(self, Self::Windsurf)
    }

    pub fn includes_codex(self) -> bool {
        matches!(self, Self::Codex)
    }
}

/// Supported project technology stacks detected in target repositories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackKind {
    React,
    Vue,
    Svelte,
    Vite,
    NextJs,
    Astro,
    Angular,
    NestJs,
    ExpoReactNative,
    TypeScript,
    Python,
    KotlinAndroid,
    FlutterDart,
    Rust,
    Php,
    Laravel,
    Godot,
}

/// Supported linters, typecheckers, and QA tools detected in a repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Eslint,
    Biome,
    Ruff,
    Tsc,
    VueTsc,
    Pyright,
    Mypy,
    Ktlint,
    Detekt,
    DartAnalyze,
    Vitest,
    Jest,
    Pytest,
    Roborazzi,
    SonarQube,
    Cargo,
    Gradle,
    Composer,
    PhpStan,
    Psalm,
    Pint,
    PhpCsFixer,
    PhpUnit,
    Pest,
    Godot,
    GdLint,
    GdUnit4,
    Gut,
}

/// Configuration for a single repository or subrepo within a multi-repo workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubrepoConfig {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub stacks: Vec<StackKind>,
    #[serde(default)]
    pub tools: Vec<ToolKind>,
    #[serde(default)]
    pub sonar_enabled: bool,
}

/// Latency and context-budget limits for Tier 0/1/2 execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierBudgets {
    #[serde(default = "default_tier0_ms")]
    pub tier0_ms: u64,
    /// Latency target for a Tier 1 pass.
    #[serde(default = "default_tier1_secs")]
    pub tier1_secs: u64,
    /// Hard wall-clock limit per Tier 1 typecheck or test process (cold builds included).
    #[serde(default = "default_tier1_timeout_secs")]
    pub tier1_timeout_secs: u64,
    #[serde(default = "default_max_lines")]
    pub compressor_max_lines: usize,
    #[serde(default = "default_max_tokens")]
    pub compressor_max_tokens: usize,
}

impl Default for TierBudgets {
    fn default() -> Self {
        Self {
            tier0_ms: default_tier0_ms(),
            tier1_secs: default_tier1_secs(),
            tier1_timeout_secs: default_tier1_timeout_secs(),
            compressor_max_lines: default_max_lines(),
            compressor_max_tokens: default_max_tokens(),
        }
    }
}

impl TierBudgets {
    /// Returns the hard per-process Tier 1 timeout, never below the latency target.
    pub fn tier1_timeout(&self) -> Duration {
        Duration::from_secs(self.tier1_timeout_secs.max(self.tier1_secs).max(5))
    }
}

/// Web visual QA driver selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebQaDriver {
    #[default]
    Unconfigured,
    ChromeDevTools,
    Playwright,
    Browser,
    Skip,
}

/// Mobile visual QA driver selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileQaDriver {
    #[default]
    Unconfigured,
    AdbMobileMcp,
    Roborazzi,
    Maestro,
    Skip,
}

/// Visual QA driver preferences persisted per repository.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct QaDriverConfig {
    #[serde(default)]
    pub web: WebQaDriver,
    #[serde(default)]
    pub mobile: MobileQaDriver,
}

/// Architecture boundary rule forbidding imports from `from_glob` into `forbid_glob`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundaryRule {
    pub name: String,
    pub from_glob: String,
    pub forbid_glob: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Opt-in local SonarQube integration configuration (`enabled = false` by default).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SonarConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_key: Option<String>,
    #[serde(default = "default_sonar_token_env")]
    pub token_env: String,
    #[serde(default = "default_cognitive_max")]
    pub cognitive_complexity_max: usize,
    #[serde(default = "default_duplication_min_lines")]
    pub duplication_min_lines: usize,
}

impl Default for SonarConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host_url: None,
            project_key: None,
            token_env: default_sonar_token_env(),
            cognitive_complexity_max: default_cognitive_max(),
            duplication_min_lines: default_duplication_min_lines(),
        }
    }
}

/// Configurable thresholds for `PreToolUse` Token Guard file read checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenGuardConfig {
    #[serde(default = "default_tg_max_text_lines")]
    pub max_text_lines: usize,
    #[serde(default = "default_tg_max_read_lines")]
    pub max_read_lines: usize,
    #[serde(default = "default_tg_max_noise_read_lines")]
    pub max_noise_read_lines: usize,
    #[serde(default = "default_tg_max_binary_bytes")]
    pub max_binary_bytes: u64,
    #[serde(default = "default_tg_max_pdf_bytes")]
    pub max_pdf_bytes: u64,
    #[serde(default = "default_tg_max_pdf_pages")]
    pub max_pdf_pages: usize,
}

impl Default for TokenGuardConfig {
    fn default() -> Self {
        Self {
            max_text_lines: default_tg_max_text_lines(),
            max_read_lines: default_tg_max_read_lines(),
            max_noise_read_lines: default_tg_max_noise_read_lines(),
            max_binary_bytes: default_tg_max_binary_bytes(),
            max_pdf_bytes: default_tg_max_pdf_bytes(),
            max_pdf_pages: default_tg_max_pdf_pages(),
        }
    }
}

/// Top-level `.harness/config.json` configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnvilConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub stacks: Vec<StackKind>,
    #[serde(default)]
    pub tools: Vec<ToolKind>,
    #[serde(default)]
    pub subrepos: Vec<SubrepoConfig>,
    #[serde(default)]
    pub tier_budgets: TierBudgets,
    #[serde(default = "default_root_allowlist")]
    pub root_allowlist: Vec<String>,
    #[serde(default = "default_circuit_breaker_limit")]
    pub circuit_breaker_limit: u32,
    #[serde(default = "default_deps_max_age_months")]
    pub deps_max_age_months: u32,
    #[serde(default)]
    pub qa_driver: QaDriverConfig,
    #[serde(default)]
    pub adapters: AdapterTarget,
    #[serde(default = "default_allowed_escapes")]
    pub allowed_escapes: Vec<String>,
    #[serde(default)]
    pub boundaries: Vec<BoundaryRule>,
    #[serde(default)]
    pub forbid_import_cycles: bool,
    #[serde(default)]
    pub sonar: SonarConfig,
    /// Normalized-line window of the duplication check; falls back to `sonar.duplication_min_lines`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplication_min_lines: Option<usize>,
    #[serde(default)]
    pub token_guard: TokenGuardConfig,
    /// Destructive-command policy of the `PreToolUse` security guard.
    #[serde(default)]
    pub security_guard: SecurityGuardConfig,
    /// Engine behind `anvil grep` and the MCP `grep` tool.
    #[serde(default)]
    pub search: SearchConfig,
    /// Living documentation (OKF bundles under `.harness/docs/`).
    #[serde(default)]
    pub docs: DocsConfig,
    /// Orchestrator/executor model pair; absent until `anvil orchestrator setup` runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator: Option<OrchestratorConfig>,
    /// Language of the messages Anvil shows the user (`en`, `es`); detected when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

impl Default for AnvilConfig {
    fn default() -> Self {
        Self {
            version: default_version(),
            stacks: Vec::new(),
            tools: Vec::new(),
            subrepos: Vec::new(),
            tier_budgets: TierBudgets::default(),
            root_allowlist: default_root_allowlist(),
            circuit_breaker_limit: default_circuit_breaker_limit(),
            deps_max_age_months: default_deps_max_age_months(),
            qa_driver: QaDriverConfig::default(),
            adapters: AdapterTarget::default(),
            allowed_escapes: default_allowed_escapes(),
            boundaries: Vec::new(),
            forbid_import_cycles: false,
            sonar: SonarConfig::default(),
            duplication_min_lines: None,
            token_guard: TokenGuardConfig::default(),
            security_guard: SecurityGuardConfig::default(),
            search: SearchConfig::default(),
            docs: DocsConfig::default(),
            orchestrator: None,
            language: None,
        }
    }
}

impl AnvilConfig {
    /// Window of normalized lines the duplication check compares (at least 4).
    pub fn duplication_window(&self) -> usize {
        self.duplication_min_lines.unwrap_or(self.sonar.duplication_min_lines).max(4)
    }

    /// Returns the canonical path to `.harness/config.json`.
    pub fn path(repo_root: &Path) -> PathBuf {
        repo_root.join(".harness").join("config.json")
    }

    /// Loads `.harness/config.json`, failing if the file does not exist or is invalid.
    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = Self::path(repo_root);
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        let clean = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
        serde_json::from_str(clean)
            .with_context(|| format!("failed to parse JSON config at {}", path.display()))
    }

    /// Loads `.harness/config.json` if present, or returns defaults when absent.
    pub fn load_or_default(repo_root: &Path) -> Result<Self> {
        let path = Self::path(repo_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        Self::load(repo_root)
    }

    /// Serializes and writes `.harness/config.json`, creating `.harness/` if needed.
    pub fn save(&self, repo_root: &Path) -> Result<()> {
        let path = Self::path(repo_root);
        let json = serde_json::to_string_pretty(self).context("failed to serialize config")?;
        crate::fs_util::atomic_write(&path, format!("{json}\n").as_bytes())
    }
}

fn default_version() -> u32 {
    1
}

fn default_tier0_ms() -> u64 {
    500
}

fn default_tier1_secs() -> u64 {
    15
}

fn default_tier1_timeout_secs() -> u64 {
    300
}

fn default_max_lines() -> usize {
    50
}

fn default_max_tokens() -> usize {
    1200
}

fn default_circuit_breaker_limit() -> u32 {
    3
}

fn default_deps_max_age_months() -> u32 {
    24
}

fn default_allowed_escapes() -> Vec<String> {
    vec!["anvil-allow".to_owned()]
}

fn default_sonar_token_env() -> String {
    "SONAR_TOKEN".to_owned()
}

fn default_cognitive_max() -> usize {
    15
}

fn default_duplication_min_lines() -> usize {
    8
}

fn default_tg_max_text_lines() -> usize {
    800
}

fn default_tg_max_read_lines() -> usize {
    400
}

fn default_tg_max_noise_read_lines() -> usize {
    200
}

fn default_tg_max_binary_bytes() -> u64 {
    5 * 1024 * 1024
}

fn default_tg_max_pdf_bytes() -> u64 {
    10 * 1024 * 1024
}

fn default_tg_max_pdf_pages() -> usize {
    20
}

fn default_root_allowlist() -> Vec<String> {
    [
        ".agents",
        ".claude",
        ".gitattributes",
        ".gitignore",
        ".harness",
        ".mcp.json",
        "AGENTS.md",
        "CLAUDE.md",
        "LICENSE",
        "README.md",
        "tasks.json",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
