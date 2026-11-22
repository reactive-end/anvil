//! Schema and persistence for `.harness/config.json`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Supported project technology stacks detected in target repositories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackKind {
    React,
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
}

/// Supported linters, typecheckers, and QA tools detected in a repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Eslint,
    Biome,
    Ruff,
    Tsc,
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
}

/// Latency and context-budget limits for Tier 0/1/2 execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierBudgets {
    #[serde(default = "default_tier0_ms")]
    pub tier0_ms: u64,
    #[serde(default = "default_tier1_secs")]
    pub tier1_secs: u64,
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
            compressor_max_lines: default_max_lines(),
            compressor_max_tokens: default_max_tokens(),
        }
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
    #[serde(default = "default_allowed_escapes")]
    pub allowed_escapes: Vec<String>,
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
            allowed_escapes: default_allowed_escapes(),
        }
    }
}

impl AnvilConfig {
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
