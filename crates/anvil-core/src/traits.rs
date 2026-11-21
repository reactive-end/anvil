//! Shared domain types and traits for parallel compilation across `anvil` crates.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Severity level for diagnostics emitted by AST rules or external tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// A deterministic finding tied to a source location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub rule: String,
    pub severity: Severity,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub message: String,
    pub suggestion: Option<String>,
}

/// Supported programming and markup languages across target stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    TypeScript,
    Tsx,
    JavaScript,
    Jsx,
    Html,
    Css,
    Scss,
    Python,
    Kotlin,
    Dart,
    Json,
    Yaml,
    Markdown,
    Other,
}

/// Kind of symbol extracted from an AST.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Class,
    Interface,
    TypeAlias,
    Component,
    Module,
    Method,
    Variable,
    Constant,
    Unknown,
}

/// A symbol definition extracted from a single file parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolDef {
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub file: String,
    pub line: u32,
    pub signature: Option<String>,
    pub exported: bool,
}

/// Relationship kind between files or symbols in the code graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Imports,
    Calls,
    UsesProp,
    RendersTemplate,
    Provides,
    Injects,
    Extends,
    Implements,
    ReExports,
}

/// Directed dependency or usage edge extracted from an AST.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeRef {
    pub source_file: String,
    pub source_symbol: Option<String>,
    pub target_path_or_symbol: String,
    pub kind: EdgeKind,
    pub line: u32,
}

/// Complete single-parse AST summary consumed by both Tier 0 rules and `anvil-graph`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAstFacts {
    pub path: String,
    pub blake3_hash: String,
    pub language: Language,
    pub symbols: Vec<SymbolDef>,
    pub edges: Vec<EdgeRef>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Trait implemented by language-specific AST extractors in `anvil-ast`.
pub trait LanguageExtractor: Send + Sync {
    fn language(&self) -> Language;
    fn extract(&self, path: &Path, source: &str) -> Result<FileAstFacts>;
}

/// Freshness state of a persistent memory observation against the live code graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessState {
    Fresh,
    NeedsVerification,
    Stale,
}

/// Result of checking whether files or symbols linked to a memory remain valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshnessReport {
    pub state: FreshnessState,
    pub reason: Option<String>,
}

/// Decouples `anvil-memory` staleness checks from `anvil-graph` without circular deps.
pub trait SymbolFreshnessProvider: Send + Sync {
    fn check_freshness(
        &self,
        related_files: &[String],
        related_symbols: &[String],
    ) -> Result<FreshnessReport>;
}

/// Execution tier determined from diff classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Tier0,
    Tier1,
    Tier2,
}

/// Semantic category of a git diff used to select minimum verification tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffCategory {
    Docs,
    StyleOnly,
    Config,
    Logic,
    Contract,
    Ui,
    Deps,
}

/// Token-bounded compressed representation of a build/test/linter command run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompressedOutput {
    pub tool: String,
    pub passed: bool,
    pub exit_code: i32,
    pub summary: String,
    pub lines: Vec<String>,
    pub total_errors: usize,
    pub truncated: bool,
    pub raw_log_path: Option<PathBuf>,
}

/// Tool-specific log parser for `anvil run`.
pub trait OutputParser: Send + Sync {
    fn name(&self) -> &'static str;
    fn matches(&self, command: &str) -> bool;
    fn parse(&self, command: &str, exit_code: i32, raw_output: &str) -> CompressedOutput;
}
