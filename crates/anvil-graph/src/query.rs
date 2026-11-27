//! Token-scored BM25 graph query, enclosing-symbol lookup, and structural tracing re-exports.

mod enclosing;
mod origin;
mod outline;
mod symbol_read;
mod trace;

use anvil_core::traits::SymbolKind;
use serde::{Deserialize, Serialize};

use crate::model::{CodeGraph, NodeId, NodeKind};

pub use enclosing::{annotate_grep_report, enclosing_symbol, symbol_label};
pub use origin::{OriginResult, find_origin};
pub use outline::{FileOutline, OutlineSymbol, RepoOutline, outline};
pub use symbol_read::{SymbolSlice, read_symbol};
pub use trace::{ExplainResult, NeighborEdge, PathResult, PathStep, explain, shortest_path};

/// Ranked node hit returned by `query`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryHit {
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    pub file: String,
    pub line: u32,
    pub score: f64,
}

struct QueryScan {
    live_docs: usize,
    total_dl: usize,
    df: Vec<usize>,
    candidates: Vec<(NodeId, usize, Vec<u32>)>,
}

/// Searches symbols, files, and semantic nodes using BM25/token scoring within `limit`.
pub fn query(graph: &CodeGraph, term: &str, limit: usize) -> Vec<QueryHit> {
    let query_tokens = collect_tokens(term);
    if query_tokens.is_empty() || limit == 0 || graph.nodes.is_empty() {
        return Vec::new();
    }
    let scan = scan_query_candidates(graph, &query_tokens);
    if scan.candidates.is_empty() {
        return Vec::new();
    }

    let mut hits = score_candidates(graph, term, &scan);
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    hits.truncate(limit);
    hits
}

fn scan_query_candidates(graph: &CodeGraph, query_tokens: &[String]) -> QueryScan {
    let q_len = query_tokens.len();
    let mut df = vec![0usize; q_len];
    let mut seen = vec![false; q_len];
    let mut tfs = vec![0u32; q_len];
    let mut tok_buf = String::with_capacity(64);

    let mut live_docs = 0usize;
    let mut total_dl = 0usize;
    let mut candidates = Vec::new();

    for (idx, node) in graph.nodes.iter().enumerate() {
        if node.is_tombstone() {
            continue;
        }
        live_docs += 1;
        seen.fill(false);
        tfs.fill(0);
        let mut dl = 0usize;
        let mut matched_any = false;

        let mut match_field = |text: &str| {
            for_each_token(text, &mut tok_buf, |tok| {
                dl += 1;
                for (qi, qt) in query_tokens.iter().enumerate() {
                    if qt == tok {
                        tfs[qi] += 1;
                        seen[qi] = true;
                        matched_any = true;
                    }
                }
            });
        };
        match_field(&node.label);
        match_field(&node.file);

        total_dl += dl;
        if matched_any {
            for (qi, &is_seen) in seen.iter().enumerate() {
                if is_seen {
                    df[qi] += 1;
                }
            }
            candidates.push((idx as u32, dl, tfs.clone()));
        }
    }
    QueryScan { live_docs, total_dl, df, candidates }
}

fn score_candidates(graph: &CodeGraph, term: &str, scan: &QueryScan) -> Vec<QueryHit> {
    let n_docs = scan.live_docs.max(1) as f64;
    let avg_len = (scan.total_dl as f64 / n_docs).max(1.0);
    let term_lower = term.trim().to_ascii_lowercase();

    scan.candidates
        .iter()
        .filter_map(|(nid, dl, node_tfs)| {
            let node = graph.get_node(*nid)?;
            let mut score = bm25_score(node_tfs, *dl, &scan.df, n_docs, avg_len);
            if score <= 0.0 {
                return None;
            }
            if node.label.eq_ignore_ascii_case(&term_lower) {
                score += 10.0;
            } else if node.file.to_ascii_lowercase().ends_with(&term_lower) {
                score += 5.0;
            }
            if node.exported {
                score += 0.5;
            }
            Some(QueryHit {
                id: node.id.clone(),
                kind: node.kind,
                label: node.label.clone(),
                file: node.file.clone(),
                line: node.line,
                score,
            })
        })
        .collect()
}

fn bm25_score(tfs: &[u32], doc_len: usize, df: &[usize], n_docs: f64, avg_len: f64) -> f64 {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;
    let dl = doc_len as f64;
    let mut total = 0.0;

    for (&tf_u32, &doc_freq_usize) in tfs.iter().zip(df.iter()) {
        if tf_u32 == 0 {
            continue;
        }
        let tf = tf_u32 as f64;
        let doc_freq = doc_freq_usize as f64;
        let idf = ((n_docs - doc_freq + 0.5) / (doc_freq + 0.5) + 1.0).ln();
        let num = tf * (K1 + 1.0);
        let den = tf + K1 * (1.0 - B + B * (dl / avg_len));
        total += idf * (num / den);
    }
    total
}

/// Resolves an arbitrary node ID, repo-relative file path, or symbol label to a `NodeId`.
pub fn resolve_target_node(graph: &CodeGraph, query_str: &str) -> Option<NodeId> {
    if let Some((nid, _)) = graph.node_by_id(query_str) {
        return Some(nid);
    }
    let norm = CodeGraph::normalize_rel_path(query_str);
    let file_id = CodeGraph::file_node_id(&norm);
    if let Some((nid, _)) = graph.node_by_id(&file_id) {
        return Some(nid);
    }
    if let Some(ids) = graph.symbol_index.get(query_str)
        && let Some(&first) = ids.iter().find(|&&id| graph.get_node(id).is_some())
    {
        return Some(first);
    }
    graph
        .nodes
        .iter()
        .enumerate()
        .find(|(_, n)| {
            !n.is_tombstone()
                && ((n.kind == NodeKind::Symbol && n.label.eq_ignore_ascii_case(query_str))
                    || (n.kind == NodeKind::File && n.file.ends_with(&norm)))
        })
        .map(|(idx, _)| idx as u32)
}

fn collect_tokens(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::with_capacity(32);
    for_each_token(input, &mut buf, |tok| out.push(tok.to_owned()));
    out
}

fn for_each_token(input: &str, buf: &mut String, mut on_tok: impl FnMut(&str)) {
    buf.clear();
    let mut prev_lower = false;
    for ch in input.chars() {
        if !ch.is_alphanumeric() {
            if !buf.is_empty() {
                on_tok(buf);
                buf.clear();
            }
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && !buf.is_empty() && prev_lower {
            on_tok(buf);
            buf.clear();
        }
        prev_lower = ch.is_lowercase();
        if ch.is_ascii() {
            buf.push(ch.to_ascii_lowercase());
        } else {
            for lc in ch.to_lowercase() {
                buf.push(lc);
            }
        }
    }
    if !buf.is_empty() {
        on_tok(buf);
        buf.clear();
    }
}

pub(crate) fn symbol_kind_str(kind: Option<SymbolKind>) -> &'static str {
    match kind {
        Some(SymbolKind::Function) => "function",
        Some(SymbolKind::Class) => "class",
        Some(SymbolKind::Interface) => "interface",
        Some(SymbolKind::TypeAlias) => "type_alias",
        Some(SymbolKind::Component) => "component",
        Some(SymbolKind::Module) => "module",
        Some(SymbolKind::Method) => "method",
        Some(SymbolKind::Variable) => "variable",
        Some(SymbolKind::Constant) => "constant",
        Some(SymbolKind::Unknown) | None => "unknown",
    }
}
