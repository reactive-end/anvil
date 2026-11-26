//! Compact body-free file and directory outline (`graph_outline`) with token-budget enforcement.

use std::fs;
use std::path::Path;

use anvil_core::stats::estimate_tokens;
use serde::{Deserialize, Serialize};

use crate::model::{CodeGraph, GraphEdgeKind, GraphNode, NodeId, NodeKind};
use crate::query::symbol_kind_str;

/// Compact signature-only symbol summary inside a `FileOutline`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutlineSymbol {
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub end_line: u32,
    pub signature: Option<String>,
    pub exported: bool,
}

/// Compact outline for a single source file (symbols + exposed HTTP routes, zero bodies).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOutline {
    pub file: String,
    pub symbols: Vec<OutlineSymbol>,
    pub routes: Vec<String>,
}

/// Budget-bounded repository or directory outline returned by `outline`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoOutline {
    pub files: Vec<FileOutline>,
    pub files_total: usize,
    pub truncated: bool,
    pub estimated_tokens_saved: u64,
}

/// Builds a compact signature-only outline for `target` (file, directory prefix, or entire repo).
pub fn outline(
    graph: &CodeGraph,
    repo_root: &Path,
    target: Option<&str>,
    max_tokens: usize,
) -> RepoOutline {
    let mut matching = collect_matching_files(graph, target);
    matching.sort_by(|a, b| a.0.cmp(&b.0));
    let files_total = matching.len();

    let mut files = Vec::new();
    let mut used_tokens = 0usize;
    let mut outline_bytes = 0usize;
    let mut raw_bytes = 0usize;
    let mut truncated = false;

    for (rel_path, file_nid) in matching {
        let entry = build_file_outline(graph, &rel_path, file_nid);
        let entry_bytes = serde_json::to_string(&entry).map_or(0, |s| s.len());
        let entry_tokens = estimate_tokens(entry_bytes) as usize;
        if used_tokens + entry_tokens > max_tokens {
            truncated = true;
            break;
        }
        used_tokens += entry_tokens;
        outline_bytes += entry_bytes;
        raw_bytes += file_byte_size(repo_root, graph, &rel_path);
        files.push(entry);
    }

    let estimated_tokens_saved =
        estimate_tokens(raw_bytes).saturating_sub(estimate_tokens(outline_bytes));
    RepoOutline { files, files_total, truncated, estimated_tokens_saved }
}

fn collect_matching_files(graph: &CodeGraph, target: Option<&str>) -> Vec<(String, NodeId)> {
    let norm_filter = target
        .map(|t| CodeGraph::normalize_rel_path(t.trim()).trim_end_matches('/').to_owned())
        .filter(|s| !s.is_empty() && s != ".");
    let dir_prefix = norm_filter.as_ref().map(|p| format!("{p}/"));

    graph
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(idx, node)| {
            if node.is_tombstone() || node.kind != NodeKind::File {
                return None;
            }
            if let (Some(exact), Some(prefix)) = (&norm_filter, &dir_prefix)
                && node.file != *exact
                && !node.file.starts_with(prefix)
            {
                return None;
            }
            Some((node.file.clone(), idx as NodeId))
        })
        .collect()
}

fn build_file_outline(graph: &CodeGraph, rel_path: &str, file_nid: NodeId) -> FileOutline {
    let nids = graph.file_nodes.get(rel_path).map(Vec::as_slice).unwrap_or(&[]);
    let all_syms: Vec<&GraphNode> = nids
        .iter()
        .filter_map(|&nid| graph.get_node(nid))
        .filter(|n| n.kind == NodeKind::Symbol)
        .collect();

    let selected = select_outline_symbols(&all_syms);
    let mut symbols: Vec<OutlineSymbol> = selected
        .into_iter()
        .map(|n| OutlineSymbol {
            name: n.label.clone(),
            kind: symbol_kind_str(n.symbol_kind).to_owned(),
            line: n.line,
            end_line: n.end_line,
            signature: n.signature.clone(),
            exported: n.exported,
        })
        .collect();
    symbols.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.name.cmp(&b.name)));

    let routes = collect_file_routes(graph, file_nid, nids);
    FileOutline { file: rel_path.to_owned(), symbols, routes }
}

fn select_outline_symbols<'a>(all_syms: &[&'a GraphNode]) -> Vec<&'a GraphNode> {
    let exported: Vec<&GraphNode> = all_syms.iter().copied().filter(|n| n.exported).collect();
    if !exported.is_empty() {
        return exported;
    }
    let top_level: Vec<&GraphNode> = all_syms
        .iter()
        .copied()
        .filter(|n| n.id.split_once("::").is_some_and(|(_, qname)| !qname.contains('.')))
        .collect();
    if !top_level.is_empty() { top_level } else { all_syms.to_vec() }
}

fn collect_file_routes(graph: &CodeGraph, file_nid: NodeId, nids: &[NodeId]) -> Vec<String> {
    let mut routes = Vec::new();
    let mut push_routes_for = |nid: NodeId| {
        for edge in graph.incoming_edges(nid) {
            if edge.kind == GraphEdgeKind::ExposesRoute
                && let Some(src) = graph.get_node(edge.source)
                && !routes.contains(&src.label)
            {
                routes.push(src.label.clone());
            }
        }
    };
    push_routes_for(file_nid);
    for &nid in nids {
        if nid != file_nid {
            push_routes_for(nid);
        }
    }
    routes.sort();
    routes
}

fn file_byte_size(repo_root: &Path, graph: &CodeGraph, rel_path: &str) -> usize {
    if let Ok(meta) = fs::metadata(repo_root.join(rel_path)) {
        return usize::try_from(meta.len()).unwrap_or(usize::MAX);
    }
    graph.file_stats.get(rel_path).map_or(0, |&(_, len)| usize::try_from(len).unwrap_or(usize::MAX))
}
