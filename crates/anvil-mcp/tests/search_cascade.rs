//! The graph -> docs -> code cascade: stage selection, rendering, budget, and missing inputs.

use std::fs;
use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_graph::CodeGraphEngine;
use anvil_mcp::search_cascade::{CascadeOptions, CodeStage, Stage, cascade, render};
use tempfile::TempDir;

const FORMATTERS: &str = "export function formatDate(d: Date): string {\n  return d.toISOString();\n}\n\nexport function formatMoney(n: number): string {\n  return n.toFixed(2);\n}\n\nexport function formatName(n: string): string {\n  return n.trim();\n}\n";
const CONCEPT: &str = "---\ntype: Code Module\ntitle: Invoicing\ndescription: How invoices are totalled\n---\n# Invoicing\n\nInvoices are totalled by the ledger service.\n";

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/format.ts", FORMATTERS);
    write(dir.path(), "notes.md", "The zebra crossing is on the left.\n");
    dir
}

fn with_graph(dir: &TempDir) {
    CodeGraphEngine::build(dir.path(), AnvilConfig::default()).unwrap().save().unwrap();
}

fn with_docs(dir: &TempDir) {
    write(dir.path(), ".harness/docs/shop/invoicing/overview.md", CONCEPT);
}

fn run(
    dir: &TempDir,
    query: &str,
    opts: &CascadeOptions,
) -> anvil_mcp::search_cascade::CascadeReport {
    cascade(dir.path(), &AnvilConfig::default(), query, opts)
}

#[test]
fn graph_hits_answer_without_running_the_code_stage() {
    let dir = repo();
    with_graph(&dir);
    let report = run(&dir, "format", &CascadeOptions::default());

    assert_eq!(report.answered_by, Some(Stage::Graph));
    assert!(report.graph.hits.len() >= 3, "{:?}", report.graph.hits);
    assert!(report.code.report.is_none(), "code must not run when the graph answers");
    let text = render(&report);
    assert!(text.starts_with("# search \"format\": answered by graph"), "{text}");
    let (graph_at, docs_at, code_at) = (
        text.find("## graph").unwrap(),
        text.find("## docs").unwrap(),
        text.find("## code").unwrap(),
    );
    assert!(graph_at < docs_at && docs_at < code_at, "sections out of order:\n{text}");
    assert!(text.contains("symbol formatDate  src/format.ts:1"), "{text}");
    assert!(text.contains("next: `graph_read_symbol "), "{text}");
}

#[test]
fn no_graph_or_docs_hits_runs_the_code_stage() {
    let dir = repo();
    with_graph(&dir);
    with_docs(&dir);
    let report = run(&dir, "zebra", &CascadeOptions::default());

    assert!(report.graph.hits.is_empty() && report.docs.hits.is_empty());
    let code = report.code.report.as_ref().expect("code stage ran");
    assert_eq!(code.total_matches, 1);
    assert_eq!(report.answered_by, Some(Stage::Code));
    let text = render(&report);
    assert!(text.contains("answered by code"), "{text}");
    assert!(text.contains("notes.md\n  1: The zebra crossing is on the left."), "{text}");
    assert!(text.contains("next: `grep` with a narrower pattern"), "{text}");
}

#[test]
fn literal_queries_run_the_code_stage_even_when_the_graph_answers() {
    let dir = repo();
    with_graph(&dir);
    let report = run(&dir, "formatDate(", &CascadeOptions::default());

    assert!(report.graph.hits.len() >= 3);
    let code = report.code.report.as_ref().expect("literal query greps");
    assert_eq!(code.total_matches, 1, "fixed-string match of `formatDate(`");
    assert_eq!(report.answered_by, Some(Stage::Graph), "graph still answers first");

    let quoted = run(&dir, "\"toISOString\"", &CascadeOptions::default());
    assert_eq!(quoted.query, "toISOString");
    assert_eq!(quoted.code.report.as_ref().expect("quoted greps").total_matches, 1);
}

#[test]
fn code_can_be_forced_or_skipped() {
    let dir = repo();
    with_graph(&dir);
    let forced =
        run(&dir, "format", &CascadeOptions { code: CodeStage::Force, ..Default::default() });
    assert!(forced.code.report.is_some_and(|r| r.total_matches >= 3));
    let skipped =
        run(&dir, "zebra", &CascadeOptions { code: CodeStage::Skip, ..Default::default() });
    assert!(skipped.code.report.is_none());
    assert_eq!(skipped.answered_by, None);
    assert!(render(&skipped).contains("no results in graph, docs, or code"));
}

#[test]
fn docs_answer_when_the_graph_has_no_hit() {
    let dir = repo();
    with_graph(&dir);
    with_docs(&dir);
    let report = run(&dir, "invoices totalled", &CascadeOptions::default());

    assert_eq!(report.answered_by, Some(Stage::Docs));
    assert_eq!(report.docs.hits[0].id, "shop/invoicing");
    let text = render(&report);
    assert!(
        text.contains("## docs (1)\n  shop/invoicing  .harness/docs/shop/invoicing/overview.md"),
        "{text}"
    );
    assert!(text.contains("next: `Read .harness/docs/shop/invoicing/overview.md`"), "{text}");
}

#[test]
fn missing_graph_and_docs_are_one_line_notes_not_errors() {
    let dir = repo();
    let report = run(&dir, "zebra", &CascadeOptions::default());

    assert!(report.nothing_to_search());
    let text = render(&report);
    assert!(text.contains("## graph: no code graph (run `anvil graph build`)"), "{text}");
    assert!(text.contains("## docs: no docs bundle under .harness/docs/"), "{text}");
    assert!(text.contains("notes.md"), "code still runs: {text}");
}

#[test]
fn rendering_respects_the_token_budget_and_says_when_it_cut() {
    let dir = repo();
    with_graph(&dir);
    with_docs(&dir);
    let big = run(&dir, "format", &CascadeOptions { code: CodeStage::Force, ..Default::default() });
    let full = render(&big);

    let tight = CascadeOptions { code: CodeStage::Force, max_tokens: 100, ..Default::default() };
    let small = run(&dir, "format", &tight);
    let text = render(&small);
    assert!(text.len() <= 100 * 4, "{} chars:\n{text}", text.len());
    assert!(text.len() < full.len());
    assert!(text.contains("cut to the token budget"), "{text}");
    assert!(text.contains("# search \"format\""), "header always survives: {text}");
    assert!(text.contains("\nnext: "), "next step always survives: {text}");
}
