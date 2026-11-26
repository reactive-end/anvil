//! Hyperedge ingestion helper for Graphify NetworkX overlay.

use rustc_hash::FxHashMap;
use serde::Deserialize;

use crate::graphify_compat::find_matching_file_node;
use crate::model::{CodeGraph, CompactEdge, GraphEdgeKind, GraphNode, NodeId, NodeKind};

#[derive(Deserialize)]
pub(crate) struct RawHyperedge {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub nodes: Vec<String>,
    #[serde(default)]
    pub source_file: String,
}

pub(crate) fn ingest_hyperedges(
    graph: &mut CodeGraph,
    hyperedges: &[RawHyperedge],
    id_map: &FxHashMap<String, NodeId>,
) -> usize {
    let mut added = 0;
    for he in hyperedges {
        let sem_id_str = CodeGraph::semantic_node_id(&format!("hyperedge:{}", he.id));
        let norm_file = CodeGraph::normalize_rel_path(&he.source_file);
        let sem_nid = if let Some((existing, _)) = graph.node_by_id(&sem_id_str) {
            existing
        } else {
            graph.insert_node_with_slot(
                GraphNode {
                    id: sem_id_str,
                    kind: NodeKind::Semantic,
                    label: if he.label.is_empty() { he.id.clone() } else { he.label.clone() },
                    file: norm_file.clone(),
                    line: 1,
                    end_line: 0,
                    language: None,
                    symbol_kind: None,
                    signature: Some("graphify:hyperedge".to_owned()),
                    exported: false,
                    community: None,
                },
                None,
            )
        };
        if let Some(file_nid) = find_matching_file_node(graph, &norm_file) {
            graph.edges.push(CompactEdge {
                source: sem_nid,
                target: file_nid,
                kind: GraphEdgeKind::SemanticLink,
                line: 1,
            });
            added += 1;
        }
        for member in &he.nodes {
            if let Some(&target_nid) = id_map.get(member) {
                graph.edges.push(CompactEdge {
                    source: sem_nid,
                    target: target_nid,
                    kind: GraphEdgeKind::SemanticLink,
                    line: 1,
                });
                added += 1;
            }
        }
    }
    added
}
