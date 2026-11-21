//! Hyperedge ingestion helper for Graphify NetworkX overlay.

use rustc_hash::FxHashMap;
use serde::Deserialize;

use crate::graphify_compat::find_matching_file_node;
use crate::model::{CodeGraph, GraphEdge, GraphEdgeKind, GraphNode, NodeKind};

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
    id_map: &FxHashMap<String, String>,
) -> usize {
    let mut added = 0;
    for he in hyperedges {
        let sem_id = CodeGraph::semantic_node_id(&format!("hyperedge:{}", he.id));
        let norm_file = CodeGraph::normalize_rel_path(&he.source_file);
        if !graph.nodes.contains_key(&sem_id) {
            graph.nodes.insert(
                sem_id.clone(),
                GraphNode {
                    id: sem_id.clone(),
                    kind: NodeKind::Semantic,
                    label: if he.label.is_empty() {
                        he.id.clone()
                    } else {
                        he.label.clone()
                    },
                    file: norm_file.clone(),
                    line: 1,
                    language: None,
                    symbol_kind: None,
                    signature: Some("graphify:hyperedge".to_owned()),
                    exported: false,
                    community: None,
                },
            );
        }
        if let Some(file_node) = find_matching_file_node(graph, &norm_file) {
            graph.edges.push(GraphEdge {
                source: sem_id.clone(),
                target: file_node,
                kind: GraphEdgeKind::SemanticLink,
                line: 1,
                resolved: true,
                raw_target: he.id.clone(),
            });
            added += 1;
        }
        for member in &he.nodes {
            if let Some(target_id) = id_map.get(member) {
                graph.edges.push(GraphEdge {
                    source: sem_id.clone(),
                    target: target_id.clone(),
                    kind: GraphEdgeKind::SemanticLink,
                    line: 1,
                    resolved: true,
                    raw_target: he.id.clone(),
                });
                added += 1;
            }
        }
    }
    added
}
