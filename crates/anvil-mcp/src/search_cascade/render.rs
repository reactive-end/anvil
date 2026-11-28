//! Compact text rendering of a [`CascadeReport`]: one block per stage, one shared token budget.

use anvil_core::diff_grep::render as render_grep;
use anvil_graph::model::NodeKind;
use anvil_graph::query::QueryHit;

use super::{CHARS_PER_TOKEN, CascadeReport};

/// Characters reserved for the header, which names the answering stage.
const HEADER_RESERVE: usize = 140;
const CUT_MARK: &str = "... (cut to the token budget; narrow the query)";

/// Renders `report` within roughly `report.max_tokens`: header, graph, docs, code, next step.
pub fn render(report: &CascadeReport) -> String {
    let next = format!("next: {}", report.next_step);
    let budget = (report.max_tokens * CHARS_PER_TOKEN)
        .saturating_sub(next.len() + CUT_MARK.len() + 2)
        .max(HEADER_RESERVE);
    let mut lines = vec![header(report)];
    lines.extend(graph_lines(report));
    lines.extend(docs_lines(report));
    lines.extend(code_lines(report));

    let mut out = String::new();
    let mut cut = false;
    for line in lines {
        if !out.is_empty() && out.len() + line.len() + 1 > budget {
            cut = true;
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    if cut {
        out.push_str(CUT_MARK);
        out.push('\n');
    }
    out.push_str(&next);
    out
}

/// Characters the graph and docs sections (plus a header) take, used to size the code stage.
pub fn head_chars(report: &CascadeReport) -> usize {
    let lines = graph_lines(report).into_iter().chain(docs_lines(report));
    HEADER_RESERVE + lines.map(|l| l.len() + 1).sum::<usize>()
}

/// Suggests what to do with the result.
pub fn next_step(report: &CascadeReport) -> String {
    let symbols = report.graph.hits.iter().find(|h| h.kind == NodeKind::Symbol);
    if let Some(hit) = symbols.or(report.graph.hits.first()) {
        return match hit.kind {
            NodeKind::Symbol => format!(
                "`graph_read_symbol {}` reads just that symbol (`graph_outline {}` lists its file)",
                hit.id, hit.file
            ),
            NodeKind::File => format!("`graph_outline {}` lists the file's symbols", hit.file),
            NodeKind::Semantic => format!("`graph_explain {}` shows how it connects", hit.id),
        };
    }
    if let Some(doc) = report.docs.hits.first() {
        return format!("`Read {}` for the concept", doc.path);
    }
    if report.code.report.as_ref().is_some_and(|r| r.total_matches > 0) {
        return "`grep` with a narrower pattern or paths, then read the symbol around a match \
                (`graph_read_symbol`), not the whole file"
            .to_owned();
    }
    "no hits: rephrase with identifiers, or `grep` an exact pattern (`search --code` forces it)"
        .to_owned()
}

fn header(report: &CascadeReport) -> String {
    let counts = format!(
        "{} graph, {} docs, code {}",
        report.graph.hits.len(),
        report.docs.hits.len(),
        code_state(report)
    );
    match report.answered_by {
        Some(stage) => {
            format!("# search \"{}\": answered by {} ({counts})", report.query, stage.as_str())
        }
        None => {
            format!("# search \"{}\": no results in graph, docs, or code ({counts})", report.query)
        }
    }
}

fn code_state(report: &CascadeReport) -> String {
    match &report.code.report {
        Some(code) => format!("{} match(es)", code.total_matches),
        None if report
            .code
            .note
            .as_deref()
            .is_some_and(|n| n.starts_with("code search failed")) =>
        {
            "failed".to_owned()
        }
        None => "not searched".to_owned(),
    }
}

fn graph_lines(report: &CascadeReport) -> Vec<String> {
    let section = &report.graph;
    if let Some(note) = &section.note {
        return vec![format!("## graph: {note}")];
    }
    if section.hits.is_empty() {
        return vec!["## graph: no hits".to_owned()];
    }
    let mut lines = vec![format!("## graph ({})", section.hits.len())];
    lines.extend(section.hits.iter().map(graph_line));
    lines
}

fn graph_line(hit: &QueryHit) -> String {
    let kind = match hit.kind {
        NodeKind::File => "file",
        NodeKind::Symbol => "symbol",
        NodeKind::Semantic => "semantic",
    };
    format!("  {kind} {}  {}:{}", hit.label, hit.file, hit.line)
}

fn docs_lines(report: &CascadeReport) -> Vec<String> {
    let section = &report.docs;
    if let Some(note) = &section.note {
        return vec![format!("## docs: {note}")];
    }
    if section.hits.is_empty() {
        return vec!["## docs: no hits".to_owned()];
    }
    let mut lines = vec![format!("## docs ({})", section.hits.len())];
    for hit in &section.hits {
        lines.push(format!("  {}  {}", hit.id, hit.path));
        if !hit.snippet.is_empty() {
            lines.push(format!("    {}", hit.snippet));
        }
    }
    lines
}

fn code_lines(report: &CascadeReport) -> Vec<String> {
    let code = &report.code;
    let Some(grep) = &code.report else {
        return vec![format!("## code: {}", code.note.as_deref().unwrap_or("not searched"))];
    };
    let mut lines = vec!["## code".to_owned()];
    lines.extend(render_grep(grep, report.max_tokens).lines().map(str::to_owned));
    lines
}
