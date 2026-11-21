//! Token-scored BM25 graph query and structural tracing re-exports.

mod trace;

use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;

use crate::model::{CodeGraph, GraphNode, NodeKind};

pub use trace::{
    ExplainResult, NeighborEdge, OriginResult, PathResult, PathStep, explain, find_origin,
    shortest_path,
};

/// Ranked node hit returned by `query`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueryHit {
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    pub file: String,
    pub line: u32,
    pub score: f64,
}

/// Searches symbols, files, and semantic nodes using BM25/token scoring within `limit`.
pub fn query(graph: &CodeGraph, term: &str, limit: usize) -> Vec<QueryHit> {
    let tokens = tokenize(term);
    if tokens.is_empty() || limit == 0 {
        return Vec::new();
    }

    let n_docs = graph.nodes.len().max(1) as f64;
    let mut df: FxHashMap<&str, usize> = FxHashMap::default();
    let mut doc_tokens: Vec<(&GraphNode, Vec<String>)> = Vec::with_capacity(graph.nodes.len());

    for node in graph.nodes.values() {
        let mut fields = tokenize(&node.label);
        fields.extend(tokenize(&node.file));
        let unique: FxHashSet<&str> = fields.iter().map(String::as_str).collect();
        for tok in &tokens {
            if unique.contains(tok.as_str()) {
                *df.entry(tok.as_str()).or_insert(0) += 1;
            }
        }
        doc_tokens.push((node, fields));
    }

    let avg_len = (doc_tokens.iter().map(|(_, f)| f.len()).sum::<usize>() as f64 / n_docs).max(1.0);
    let term_lower = term.trim().to_ascii_lowercase();

    let mut hits: Vec<QueryHit> = doc_tokens
        .into_iter()
        .filter_map(|(node, fields)| {
            let mut score = bm25_score(&tokens, &fields, &df, n_docs, avg_len);
            if score <= 0.0 {
                return None;
            }
            if node.label.to_ascii_lowercase() == term_lower {
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
        .collect();

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    hits.truncate(limit);
    hits
}

fn bm25_score(
    query_tokens: &[String],
    doc_tokens: &[String],
    df: &FxHashMap<&str, usize>,
    n_docs: f64,
    avg_len: f64,
) -> f64 {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;
    let dl = doc_tokens.len() as f64;
    let mut total = 0.0;

    for qt in query_tokens {
        let tf = doc_tokens.iter().filter(|dt| *dt == qt).count() as f64;
        if tf == 0.0 {
            continue;
        }
        let doc_freq = *df.get(qt.as_str()).unwrap_or(&0) as f64;
        let idf = ((n_docs - doc_freq + 0.5) / (doc_freq + 0.5) + 1.0).ln();
        let num = tf * (K1 + 1.0);
        let den = tf + K1 * (1.0 - B + B * (dl / avg_len));
        total += idf * (num / den);
    }
    total
}

/// Resolves an arbitrary node ID, repo-relative file path, or symbol label to a node ID.
pub fn resolve_target_node(graph: &CodeGraph, query_str: &str) -> Option<String> {
    if graph.nodes.contains_key(query_str) {
        return Some(query_str.to_owned());
    }
    let norm = CodeGraph::normalize_rel_path(query_str);
    let file_id = CodeGraph::file_node_id(&norm);
    if graph.nodes.contains_key(&file_id) {
        return Some(file_id);
    }
    if let Some(ids) = graph.symbol_index.get(query_str)
        && let Some(first) = ids.first()
    {
        return Some(first.clone());
    }
    let lower = query_str.to_ascii_lowercase();
    if let Some(ids) = graph.symbol_index.get(&lower)
        && let Some(first) = ids.first()
    {
        return Some(first.clone());
    }
    graph
        .nodes
        .values()
        .find(|n| n.kind == NodeKind::File && n.file.ends_with(&norm))
        .map(|n| n.id.clone())
}

fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in input.chars() {
        if !ch.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(cur.to_ascii_lowercase());
                cur.clear();
            }
            continue;
        }
        if ch.is_uppercase()
            && !cur.is_empty()
            && cur.chars().last().is_some_and(|c| c.is_lowercase())
        {
            out.push(cur.to_ascii_lowercase());
            cur.clear();
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(cur.to_ascii_lowercase());
    }
    out
}
