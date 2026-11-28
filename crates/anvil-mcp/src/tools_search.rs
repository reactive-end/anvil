//! MCP `grep` tool (ripgrep-engine text search annotated with the enclosing graph symbol) and `search`
//! tool (graph -> docs -> code cascade).

use anvil_core::diff_grep::{GrepOptions, GrepReport, grep, render};
use anvil_graph::query::annotate_grep_report;
use anvil_graph::store;
use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::McpServer;
use crate::search_cascade::{self, CascadeOptions, CodeStage, DEFAULT_MAX_TOKENS, GRAPH_LIMIT};

const MAX_CONTEXT: u64 = 10;
const MAX_MATCHES: u64 = 500;
const MIN_TOKENS: u64 = 100;
const MAX_TOKENS: u64 = 16_000;
const MAX_LIMIT: u64 = 50;

/// Runs the `grep` or `search` tool and returns the rendered text plus the report totals.
pub fn call_search_tool(server: &mut McpServer, name: &str, args: &Value) -> Result<Value> {
    if name == "search" {
        return call_cascade(server, args);
    }
    anyhow::ensure!(name == "grep", "unknown search tool: {name}");
    let opts = options_from_args(args)?;
    let mut report = grep(server.repo_root(), &opts)?;
    annotate_with_graph(server, &mut report);
    Ok(json!({
        "text": render(&report, opts.max_tokens),
        "total_matches": report.total_matches,
        "files_with_matches": report.files_with_matches,
        "files_searched": report.files_searched,
        "truncated": report.truncated,
        "backend": report.backend,
        "note": report.note,
        "estimated_tokens_saved": report.estimated_tokens_saved,
    }))
}

/// Runs the graph -> docs -> code cascade for `args.query`.
fn call_cascade(server: &mut McpServer, args: &Value) -> Result<Value> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .filter(|q| !q.trim().is_empty())
        .context("missing or empty required string argument `query`")?;
    let mut opts = CascadeOptions {
        code: if flag(args, "code") { CodeStage::Force } else { CodeStage::Auto },
        max_tokens: bounded(args, "max_tokens", MIN_TOKENS, MAX_TOKENS)
            .unwrap_or(DEFAULT_MAX_TOKENS),
        ..CascadeOptions::default()
    };
    if let Some(limit) = bounded(args, "limit", 1, MAX_LIMIT) {
        opts.graph_limit = limit;
        opts.docs_limit = limit.min(GRAPH_LIMIT);
    }
    let root = server.repo_root().to_path_buf();
    let config = server.config().clone();
    let graph = store::default_bin_path(&root)
        .is_file()
        .then(|| server.ensure_graph().ok().map(|engine| &engine.graph))
        .flatten();
    let report = search_cascade::cascade_with_graph(&root, &config, graph, query, &opts);
    Ok(json!({
        "text": search_cascade::render(&report),
        "answered_by": report.answered_by,
        "graph_hits": report.graph.hits.len(),
        "docs_hits": report.docs.hits.len(),
        "code_matches": report.code.report.as_ref().map(|r| r.total_matches),
        "next_step": report.next_step,
    }))
}

fn options_from_args(args: &Value) -> Result<GrepOptions> {
    let pattern = args
        .get("pattern")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .context("missing or empty required string argument `pattern`")?;
    let mut opts = GrepOptions::new(pattern);
    opts.paths = string_list(args, "paths");
    opts.globs = string_list(args, "glob");
    opts.ignore_case = flag(args, "ignore_case");
    opts.fixed_strings = flag(args, "fixed_strings");
    opts.word = flag(args, "word");
    if let Some(n) = bounded(args, "context", 0, MAX_CONTEXT) {
        opts.context = n;
    }
    if let Some(n) = bounded(args, "max_matches", 1, MAX_MATCHES) {
        opts.max_matches = n;
    }
    if let Some(n) = bounded(args, "max_tokens", MIN_TOKENS, MAX_TOKENS) {
        opts.max_tokens = n;
    }
    Ok(opts)
}

fn flag(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn bounded(args: &Value, key: &str, min: u64, max: u64) -> Option<usize> {
    let raw = args.get(key).and_then(Value::as_u64)?;
    usize::try_from(raw.clamp(min, max)).ok()
}

/// Reads a string or an array of strings, dropping blanks.
fn string_list(args: &Value, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::String(one)) => vec![one.trim().to_owned()],
        Some(Value::Array(items)) => {
            items.iter().filter_map(Value::as_str).map(|s| s.trim().to_owned()).collect()
        }
        _ => Vec::new(),
    }
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect()
}

/// Adds symbol annotations only when a graph has already been built; never builds one.
fn annotate_with_graph(server: &mut McpServer, report: &mut GrepReport) {
    if !store::default_bin_path(server.repo_root()).is_file() {
        return;
    }
    if let Ok(engine) = server.ensure_graph() {
        annotate_grep_report(&engine.graph, report);
    }
}
