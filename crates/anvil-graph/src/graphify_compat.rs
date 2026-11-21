//! Optional read-only overlay ingesting semantic `concept`/`rationale` nodes from `graphify-out/graph.json`.

mod hyperedges;

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use rustc_hash::FxHashMap;
use serde::Deserialize;

use crate::model::{CodeGraph, CompactEdge, GraphEdgeKind, GraphNode, NodeId, NodeKind};
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
) -> FxHashMap<String, NodeId> {
    let mut id_map: FxHashMap<String, NodeId> = FxHashMap::default();
    for node in nodes {
        let norm_file = CodeGraph::normalize_rel_path(&node.source_file);
        if is_semantic_node(node) {
            let sem_id_str = CodeGraph::semantic_node_id(&node.id);
            let sem_nid = if let Some((existing_nid, _)) = graph.node_by_id(&sem_id_str) {
                existing_nid
            } else {
                stats.semantic_nodes_added += 1;
                graph.insert_node_with_slot(
                    GraphNode {
                        id: sem_id_str,
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
                    None,
                )
            };
            id_map.insert(node.id.clone(), sem_nid);
            if let Some(file_nid) = find_matching_file_node(graph, &norm_file) {
                graph.edges.push(CompactEdge {
                    source: sem_nid,
                    target: file_nid,
                    kind: GraphEdgeKind::SemanticLink,
                    line: 1,
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
    id_map: &FxHashMap<String, NodeId>,
    stats: &mut OverlayStats,
) {
    for link in links {
        if let (Some(&src), Some(&dst)) = (id_map.get(&link.source), id_map.get(&link.target)) {
            let is_sem = graph.get_node(src).is_some_and(|n| n.kind == NodeKind::Semantic)
                || graph.get_node(dst).is_some_and(|n| n.kind == NodeKind::Semantic);
            if is_sem {
                graph.edges.push(CompactEdge {
                    source: src,
                    target: dst,
                    kind: GraphEdgeKind::SemanticLink,
                    line: 1,
                });
                stats.semantic_edges_added += 1;
            }
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

pub(crate) fn find_matching_file_node(graph: &CodeGraph, norm_file: &str) -> Option<NodeId> {
    if norm_file.is_empty() {
        return None;
    }
    let exact = CodeGraph::file_node_id(norm_file);
    if let Some((nid, _)) = graph.node_by_id(&exact) {
        return Some(nid);
    }
    graph
        .nodes
        .iter()
        .enumerate()
        .find(|(_, n)| !n.is_tombstone() && n.kind == NodeKind::File && n.file.ends_with(norm_file))
        .map(|(idx, _)| idx as u32)
}

fn find_matching_code_node(graph: &CodeGraph, norm_file: &str, label: &str) -> Option<NodeId> {
    if !label.is_empty()
        && let Some(ids) = graph.symbol_index.get(label)
    {
        for &nid in ids {
            if let Some(n) = graph.get_node(nid)
                && (norm_file.is_empty() || n.file.ends_with(norm_file))
            {
                return Some(nid);
            }
        }
    }
    find_matching_file_node(graph, norm_file)
}
