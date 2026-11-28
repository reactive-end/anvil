//! Lookup cascade behind `anvil search`, the MCP `search` tool, and the PreToolUse search guard:
//! code graph first, then the living docs, then the code itself (grep) only when it is needed.
//!
//! Every stage is read-only and never builds anything: a missing graph or docs bundle is reported in
//! one line inside its section, never as an error.

mod literal;
mod render;

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_core::diff_grep::{GrepOptions, GrepReport, grep_with_backend};
use anvil_docs::bundle;
use anvil_docs::search::{SearchHit as DocHit, search_repo};
use anvil_graph::model::CodeGraph;
use anvil_graph::query::{QueryHit, annotate_grep_report, query};
use anvil_graph::store;
use serde::Serialize;

use literal::Literal;
pub use render::render;

/// Default token budget shared by the three sections.
pub const DEFAULT_MAX_TOKENS: usize = 1200;
/// Most graph hits listed.
pub const GRAPH_LIMIT: usize = 8;
/// Most docs hits listed.
pub const DOCS_LIMIT: usize = 5;
/// The code stage runs on its own when graph and docs together return fewer hits than this.
const CODE_TRIGGER: usize = 3;
/// Smallest budget handed to the grep engine, whatever is left of the shared one.
const MIN_CODE_TOKENS: usize = 100;
const CHARS_PER_TOKEN: usize = 4;
const CODE_MAX_MATCHES: usize = 30;
const CODE_MAX_PER_FILE: usize = 3;

/// Whether the code (grep) stage runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodeStage {
    /// Only when graph and docs return fewer than 3 hits, or the query is clearly literal text.
    #[default]
    Auto,
    /// Always.
    Force,
    /// Never (used by the hook, which only needs graph and docs).
    Skip,
}

/// Tuning of one cascade run.
#[derive(Debug, Clone)]
pub struct CascadeOptions {
    pub code: CodeStage,
    /// Approximate token budget across all sections.
    pub max_tokens: usize,
    pub graph_limit: usize,
    pub docs_limit: usize,
}

impl Default for CascadeOptions {
    fn default() -> Self {
        Self {
            code: CodeStage::Auto,
            max_tokens: DEFAULT_MAX_TOKENS,
            graph_limit: GRAPH_LIMIT,
            docs_limit: DOCS_LIMIT,
        }
    }
}

/// Stage of the cascade that produced the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Graph,
    Docs,
    Code,
}

impl Stage {
    /// Lowercase stage name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Graph => "graph",
            Self::Docs => "docs",
            Self::Code => "code",
        }
    }
}

/// Hits of one stage plus a one-line note when the stage had nothing to search or was skipped.
#[derive(Debug, Clone, Serialize)]
pub struct Section<T> {
    pub hits: Vec<T>,
    pub note: Option<String>,
}

/// Outcome of the code stage.
#[derive(Debug, Clone, Serialize)]
pub struct CodeSection {
    /// Grouped matches annotated with enclosing symbols; `None` when the stage did not run or failed.
    pub report: Option<GrepReport>,
    pub note: Option<String>,
}

/// The three ordered sections of one lookup.
#[derive(Debug, Clone, Serialize)]
pub struct CascadeReport {
    pub query: String,
    pub graph: Section<QueryHit>,
    pub docs: Section<DocHit>,
    pub code: CodeSection,
    /// First stage that returned something; `None` when all came back empty.
    pub answered_by: Option<Stage>,
    /// Suggested follow-up for the agent.
    pub next_step: String,
    pub max_tokens: usize,
}

impl CascadeReport {
    /// Graph plus docs hits.
    pub fn found(&self) -> usize {
        self.graph.hits.len() + self.docs.hits.len()
    }

    /// `true` when there is neither a graph nor a docs bundle to look into.
    pub fn nothing_to_search(&self) -> bool {
        self.graph.note.is_some() && self.docs.note.is_some()
    }
}

/// Runs the cascade, loading the cached graph from disk (never building one).
pub fn cascade(
    root: &Path,
    config: &AnvilConfig,
    query_text: &str,
    opts: &CascadeOptions,
) -> CascadeReport {
    let graph = load_cached_graph(root);
    cascade_with_graph(root, config, graph.as_ref(), query_text, opts)
}

/// Runs the cascade against an already loaded graph (`None` = no graph available).
pub fn cascade_with_graph(
    root: &Path,
    config: &AnvilConfig,
    graph: Option<&CodeGraph>,
    query_text: &str,
    opts: &CascadeOptions,
) -> CascadeReport {
    let literal = Literal::parse(query_text);
    let graph_section = graph_section(graph, &literal.text, opts.graph_limit);
    let docs_section = docs_section(root, config, &literal.text, opts.docs_limit);
    let found = graph_section.hits.len() + docs_section.hits.len();
    let run_code = match opts.code {
        CodeStage::Force => true,
        CodeStage::Skip => false,
        CodeStage::Auto => found < CODE_TRIGGER || literal.is_literal(),
    };
    let mut report = CascadeReport {
        query: literal.text.clone(),
        graph: graph_section,
        docs: docs_section,
        code: CodeSection { report: None, note: Some(skipped_note(opts.code)) },
        answered_by: None,
        next_step: String::new(),
        max_tokens: opts.max_tokens,
    };
    if run_code {
        let budget = code_budget(&report);
        report.code = code_section(root, config, graph, &literal, budget);
    }
    report.answered_by = answered_by(&report);
    report.next_step = render::next_step(&report);
    report
}

fn skipped_note(code: CodeStage) -> String {
    match code {
        CodeStage::Skip => "not searched".to_owned(),
        _ => "skipped: graph and docs answered (pass --code to search the code anyway)".to_owned(),
    }
}

fn load_cached_graph(root: &Path) -> Option<CodeGraph> {
    let bin_path = store::default_bin_path(root);
    if !bin_path.is_file() {
        return None;
    }
    store::load_binary(&bin_path).ok()
}

fn graph_section(graph: Option<&CodeGraph>, text: &str, limit: usize) -> Section<QueryHit> {
    match graph {
        Some(graph) => Section { hits: query(graph, text, limit), note: None },
        None => Section {
            hits: Vec::new(),
            note: Some("no code graph (run `anvil graph build`)".to_owned()),
        },
    }
}

fn docs_section(root: &Path, config: &AnvilConfig, text: &str, limit: usize) -> Section<DocHit> {
    if !bundle::has_bundles(root) {
        return Section {
            hits: Vec::new(),
            note: Some("no docs bundle under .harness/docs/ (run `anvil docs init`)".to_owned()),
        };
    }
    Section { hits: search_repo(root, config, text, limit), note: None }
}

/// Tokens left for the code stage once the graph and docs sections are rendered.
fn code_budget(report: &CascadeReport) -> usize {
    let used = render::head_chars(report);
    let left = (report.max_tokens * CHARS_PER_TOKEN).saturating_sub(used) / CHARS_PER_TOKEN;
    left.max(MIN_CODE_TOKENS)
}

fn code_section(
    root: &Path,
    config: &AnvilConfig,
    graph: Option<&CodeGraph>,
    literal: &Literal,
    max_tokens: usize,
) -> CodeSection {
    let mut opts = GrepOptions::new(&literal.text);
    opts.fixed_strings = !literal.looks_regex();
    opts.max_tokens = max_tokens;
    opts.max_matches = CODE_MAX_MATCHES;
    opts.max_per_file = CODE_MAX_PER_FILE;
    let backend = config.search.backend;
    let mut outcome = grep_with_backend(root, &opts, backend);
    if outcome.is_err() && !opts.fixed_strings {
        opts.fixed_strings = true;
        outcome = grep_with_backend(root, &opts, backend);
    }
    match outcome {
        Ok(mut report) => {
            if let Some(graph) = graph {
                annotate_grep_report(graph, &mut report);
            }
            CodeSection { report: Some(report), note: None }
        }
        Err(err) => {
            CodeSection { report: None, note: Some(format!("code search failed: {err:#}")) }
        }
    }
}

fn answered_by(report: &CascadeReport) -> Option<Stage> {
    if !report.graph.hits.is_empty() {
        Some(Stage::Graph)
    } else if !report.docs.hits.is_empty() {
        Some(Stage::Docs)
    } else if report.code.report.as_ref().is_some_and(|r| r.total_matches > 0) {
        Some(Stage::Code)
    } else {
        None
    }
}
