//! Graph analytics: community detection, God Nodes, import cycles, Markdown report, and static HTML.

mod cycles;
mod export;

use std::fmt::Write as _;

use rustc_hash::FxHashMap;
use serde::Serialize;

use crate::model::{CodeGraph, GraphEdgeKind, NodeKind};

pub use cycles::detect_cycles;
pub use export::export_html;

/// Summary of a detected module/feature community.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommunitySummary {
    pub id: usize,
    pub node_count: usize,
    pub sample_files: Vec<String>,
}

/// High-centrality node identified by total dependency degree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GodNode {
    pub id: String,
    pub label: String,
    pub file: String,
    pub in_degree: usize,
    pub out_degree: usize,
    pub total_degree: usize,
}

/// Assigns deterministic communities via label propagation on file dependency edges.
pub fn detect_communities(graph: &mut CodeGraph) -> Vec<CommunitySummary> {
    let mut files: Vec<String> = graph
        .nodes
        .iter()
        .filter(|n| !n.is_tombstone() && n.kind == NodeKind::File)
        .map(|n| n.file.clone())
        .collect();
    files.sort();

    let file_to_idx: FxHashMap<&str, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.as_str(), i))
        .collect();

    let mut node_to_file_idx = vec![usize::MAX; graph.nodes.len()];
    for (nid, node) in graph.nodes.iter().enumerate() {
        if !node.is_tombstone()
            && let Some(&fi) = file_to_idx.get(node.file.as_str())
        {
            node_to_file_idx[nid] = fi;
        }
    }

    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); files.len()];
    for edge in &graph.edges {
        if !edge.is_resolved() || edge.kind == GraphEdgeKind::Contains {
            continue;
        }
        let u = node_to_file_idx.get(edge.source as usize).copied().unwrap_or(usize::MAX);
        let v = node_to_file_idx.get(edge.target as usize).copied().unwrap_or(usize::MAX);
        if u != usize::MAX && v != usize::MAX && u != v {
            adj[u].push(v);
            adj[v].push(u);
        }
    }

    let mut labels: Vec<usize> = (0..files.len()).collect();
    propagate_labels(&adj, &mut labels);

    let mut remap: FxHashMap<usize, usize> = FxHashMap::default();
    let mut file_comm: FxHashMap<String, usize> = FxHashMap::default();
    for (i, f) in files.iter().enumerate() {
        let next_id = remap.len();
        let cid = *remap.entry(labels[i]).or_insert(next_id);
        file_comm.insert(f.clone(), cid);
    }

    for node in &mut graph.nodes {
        if !node.is_tombstone() {
            node.community = file_comm.get(&node.file).copied();
        }
    }
    summarize_communities(&files, &file_comm)
}

fn propagate_labels(adj: &[Vec<usize>], labels: &mut [usize]) {
    let mut counts: FxHashMap<usize, usize> = FxHashMap::default();
    for _ in 0..15 {
        let mut changed = false;
        for u in 0..adj.len() {
            if adj[u].is_empty() {
                continue;
            }
            counts.clear();
            for &v in &adj[u] {
                *counts.entry(labels[v]).or_insert(0) += 1;
            }
            let best = counts
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                .map(|(&lbl, _)| lbl)
                .unwrap_or(labels[u]);
            if best != labels[u] {
                labels[u] = best;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

fn summarize_communities(
    files: &[String],
    file_comm: &FxHashMap<String, usize>,
) -> Vec<CommunitySummary> {
    let mut groups: FxHashMap<usize, Vec<String>> = FxHashMap::default();
    for f in files {
        if let Some(&cid) = file_comm.get(f) {
            groups.entry(cid).or_default().push(f.clone());
        }
    }
    let mut summaries: Vec<CommunitySummary> = groups
        .into_iter()
        .map(|(id, mut members)| {
            members.sort();
            let node_count = members.len();
            members.truncate(5);
            CommunitySummary {
                id,
                node_count,
                sample_files: members,
            }
        })
        .collect();
    summaries.sort_by(|a, b| b.node_count.cmp(&a.node_count).then_with(|| a.id.cmp(&b.id)));
    summaries
}

/// Returns the top `top_n` highest-degree nodes (God Nodes) excluding `Contains` edges.
pub fn find_god_nodes(graph: &CodeGraph, top_n: usize) -> Vec<GodNode> {
    let mut ranked: Vec<GodNode> = graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| !n.is_tombstone() && n.kind != NodeKind::Semantic)
        .filter_map(|(nid, node)| {
            let in_deg = count_non_contain_edges(graph, graph.reverse_adj.get(nid));
            let out_deg = count_non_contain_edges(graph, graph.forward_adj.get(nid));
            let total = in_deg + out_deg;
            (total > 0).then(|| GodNode {
                id: node.id.clone(),
                label: node.label.clone(),
                file: node.file.clone(),
                in_degree: in_deg,
                out_degree: out_deg,
                total_degree: total,
            })
        })
        .collect();

    ranked.sort_by(|a, b| {
        b.total_degree
            .cmp(&a.total_degree)
            .then_with(|| a.id.cmp(&b.id))
    });
    ranked.truncate(top_n);
    ranked
}

fn count_non_contain_edges(graph: &CodeGraph, idxs: Option<&Vec<u32>>) -> usize {
    idxs.map(|v| {
        v.iter()
            .filter(|&&i| {
                graph
                    .edges
                    .get(i as usize)
                    .is_some_and(|e| !e.is_tombstone() && e.kind != GraphEdgeKind::Contains)
            })
            .count()
    })
    .unwrap_or(0)
}

/// Generates a compact Markdown architecture report (`GRAPH_REPORT.md`).
pub fn generate_report(graph: &mut CodeGraph) -> String {
    let communities = detect_communities(graph);
    let god_nodes = find_god_nodes(graph, 10);
    let cycles = detect_cycles(graph, 10);

    let file_count = graph
        .nodes
        .iter()
        .filter(|n| !n.is_tombstone() && n.kind == NodeKind::File)
        .count();
    let sym_count = graph
        .nodes
        .iter()
        .filter(|n| !n.is_tombstone() && n.kind == NodeKind::Symbol)
        .count();
    let sem_count = graph
        .nodes
        .iter()
        .filter(|n| !n.is_tombstone() && n.kind == NodeKind::Semantic)
        .count();
    let total_edges = graph.edges.iter().filter(|e| !e.is_tombstone()).count();
    let resolved_edges = graph.edges.iter().filter(|e| e.is_resolved()).count();

    let mut out = String::new();
    let _ = writeln!(out, "# Anvil Code Graph Report\n");
    let _ = writeln!(
        out,
        "- **Files**: {file_count} | **Symbols**: {sym_count} | **Semantic**: {sem_count}"
    );
    let _ = writeln!(out, "- **Edges**: {total_edges} ({resolved_edges} resolved)\n");

    let _ = writeln!(out, "## Top God Nodes (Degree Centrality)");
    for g in &god_nodes {
        let _ = writeln!(
            out,
            "- `{}` ({}) — degree {} (in: {}, out: {})",
            g.label, g.file, g.total_degree, g.in_degree, g.out_degree
        );
    }
    let _ = writeln!(out, "\n## Communities ({})", communities.len());
    for c in communities.iter().take(10) {
        let _ = writeln!(
            out,
            "- Community #{}: {} files ({})",
            c.id,
            c.node_count,
            c.sample_files.join(", ")
        );
    }
    let _ = writeln!(out, "\n## Import Cycles ({})", cycles.len());
    if cycles.is_empty() {
        let _ = writeln!(out, "- None detected.");
    } else {
        for cyc in &cycles {
            let _ = writeln!(out, "- {}", cyc.join(" -> "));
        }
    }
    out
}
