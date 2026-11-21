//! Optional read-only overlay ingesting semantic `concept`/`rationale` nodes from `graphify-out/graph.json`.

mod hyperedges;

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use rustc_hash::FxHashMap;
use serde::Deserialize;

use crate::model::{CodeGraph, GraphEdge, GraphEdgeKind, GraphNode, NodeKind};
use hyperedges::{RawHyperedge, ingest_hyperedges};

/// Statistics reported after overlaying a `graphify-out/graph.json` file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OverlayStats {
    pub semantic_nodes_added: usize,
    pub semantic_edges_added: usize,
    pub hyperedges_linked: usize,
}

#[derive(Deserialize)]
struct RawGraphifyJson {
    #[serde(default)]
    graph: Option<RawGraphMeta>,
    #[serde(default)]
    nodes: Vec<RawGraphifyNode>,
    #[serde(default)]
    links: Vec<RawGraphifyLink>,
}

#[derive(Deserialize)]
struct RawGraphMeta {
    #[serde(default)]
    hyperedges: Vec<RawHyperedge>,
}

#[derive(Deserialize)]
struct RawGraphifyNode {
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    file_type: String,
    #[serde(default)]
    source_file: String,
    #[serde(default, rename = "_origin")]
    origin: String,
}

#[derive(Deserialize)]
struct RawGraphifyLink {
    source: String,
    target: String,
    #[serde(default)]
    relation: String,
}

/// Overlays semantic/rationale nodes and hyperedges from a Graphify NetworkX JSON file without altering AST nodes.
pub fn load_graphify_overlay(graph: &mut CodeGraph, json_path: &Path) -> Result<OverlayStats> {
    let raw = fs::read_to_string(json_path)
        .with_context(|| format!("failed to read graphify JSON {}", json_path.display()))?;
    let parsed: RawGraphifyJson = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse graphify JSON {}", json_path.display()))?;

    let mut stats = OverlayStats::default();
    let id_map = ingest_semantic_nodes(graph, &parsed.nodes, &mut stats);
    ingest_semantic_links(graph, &parsed.links, &id_map, &mut stats);

    if let Some(meta) = &parsed.graph {
        stats.hyperedges_linked += ingest_hyperedges(graph, &meta.hyperedges, &id_map);
    }

    graph.rebuild_indexes();
    Ok(stats)
}

fn ingest_semantic_nodes(
    graph: &mut CodeGraph,
    nodes: &[RawGraphifyNode],
    stats: &mut OverlayStats,
) -> FxHashMap<String, String> {
    let mut id_map: FxHashMap<String, String> = FxHashMap::default();
    for node in nodes {
        let norm_file = CodeGraph::normalize_rel_path(&node.source_file);
        if is_semantic_node(node) {
            let sem_id = CodeGraph::semantic_node_id(&node.id);
            if !graph.nodes.contains_key(&sem_id) {
                graph.nodes.insert(
                    sem_id.clone(),
                    GraphNode {
                        id: sem_id.clone(),
                        kind: NodeKind::Semantic,
                        label: if node.label.is_empty() {
                            node.id.clone()
                        } else {
                            node.label.clone()
                        },
                        file: norm_file.clone(),
                        line: 1,
                        language: None,
                        symbol_kind: None,
                        signature: Some(format!("graphify:{}", node.file_type)),
                        exported: false,
                        community: None,
                    },
                );
                stats.semantic_nodes_added += 1;
            }
            id_map.insert(node.id.clone(), sem_id.clone());
            if let Some(file_node) = find_matching_file_node(graph, &norm_file) {
                graph.edges.push(GraphEdge {
                    source: sem_id,
                    target: file_node,
                    kind: GraphEdgeKind::SemanticLink,
                    line: 1,
                    resolved: true,
                    raw_target: norm_file,
                });
                stats.semantic_edges_added += 1;
            }
        } else if let Some(existing) = find_matching_code_node(graph, &norm_file, &node.label) {
            id_map.insert(node.id.clone(), existing);
        }
    }
    id_map
}

fn ingest_semantic_links(
    graph: &mut CodeGraph,
    links: &[RawGraphifyLink],
    id_map: &FxHashMap<String, String>,
    stats: &mut OverlayStats,
) {
    for link in links {
        if let (Some(src), Some(dst)) = (id_map.get(&link.source), id_map.get(&link.target))
            && (src.starts_with("sem:") || dst.starts_with("sem:"))
        {
            graph.edges.push(GraphEdge {
                source: src.clone(),
                target: dst.clone(),
                kind: GraphEdgeKind::SemanticLink,
                line: 1,
                resolved: true,
                raw_target: link.relation.clone(),
            });
            stats.semantic_edges_added += 1;
        }
    }
}

fn is_semantic_node(node: &RawGraphifyNode) -> bool {
    let ft = node.file_type.to_ascii_lowercase();
    matches!(
        ft.as_str(),
        "semantic" | "concept" | "rationale" | "document" | "doc"
    ) || node.origin.eq_ignore_ascii_case("llm")
}

pub(crate) fn find_matching_file_node(graph: &CodeGraph, norm_file: &str) -> Option<String> {
    if norm_file.is_empty() {
        return None;
    }
    let exact = CodeGraph::file_node_id(norm_file);
    if graph.nodes.contains_key(&exact) {
        return Some(exact);
    }
    graph
        .nodes
        .values()
        .find(|n| n.kind == NodeKind::File && n.file.ends_with(norm_file))
        .map(|n| n.id.clone())
}

fn find_matching_code_node(graph: &CodeGraph, norm_file: &str, label: &str) -> Option<String> {
    if !label.is_empty()
        && let Some(ids) = graph.symbol_index.get(label)
    {
        for id in ids {
            if let Some(n) = graph.nodes.get(id)
                && (norm_file.is_empty() || n.file.ends_with(norm_file))
            {
                return Some(id.clone());
            }
        }
    }
    find_matching_file_node(graph, norm_file)
}
