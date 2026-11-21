//! Multi-repo unified graph merging (e.g. `fliiper/` + `centinela/`).

use rustc_hash::FxHashMap;

use crate::model::{CodeGraph, GraphEdge, GraphEdgeKind, GraphNode, NodeKind};

/// Merges multiple named repository graphs into a single prefix-namespaced `CodeGraph`.
pub fn merge_graphs(repos: &[(&str, &CodeGraph)]) -> CodeGraph {
    let mut merged = CodeGraph::new("multi-repo");

    for &(repo_name, sub) in repos {
        let prefix = CodeGraph::normalize_rel_path(repo_name).trim_matches('/').to_owned();
        let mut id_map: FxHashMap<&str, String> = FxHashMap::default();
        id_map.reserve(sub.nodes.len());

        for (rel_path, hash) in &sub.file_hashes {
            merged
                .file_hashes
                .insert(format!("{prefix}/{rel_path}"), hash.clone());
        }

        for (old_id, node) in &sub.nodes {
            let new_file = if node.file.is_empty() {
                String::new()
            } else {
                format!("{prefix}/{}", node.file)
            };
            let new_id = prefix_node_id(&prefix, old_id, node.kind);
            id_map.insert(old_id.as_str(), new_id.clone());

            merged.nodes.insert(
                new_id.clone(),
                GraphNode {
                    id: new_id,
                    kind: node.kind,
                    label: node.label.clone(),
                    file: new_file,
                    line: node.line,
                    language: node.language,
                    symbol_kind: node.symbol_kind,
                    signature: node.signature.clone(),
                    exported: node.exported,
                    community: node.community,
                },
            );
        }

        for edge in &sub.edges {
            let source = id_map
                .get(edge.source.as_str())
                .cloned()
                .unwrap_or_else(|| prefix_node_id(&prefix, &edge.source, NodeKind::File));
            let target = id_map
                .get(edge.target.as_str())
                .cloned()
                .unwrap_or_else(|| edge.target.clone());
            merged.edges.push(GraphEdge {
                source,
                target,
                kind: edge.kind,
                line: edge.line,
                resolved: edge.resolved,
                raw_target: edge.raw_target.clone(),
            });
        }
    }

    merged.rebuild_indexes();
    link_cross_repo_symbols(&mut merged);
    merged.rebuild_indexes();
    merged
}

fn prefix_node_id(prefix: &str, old_id: &str, kind: NodeKind) -> String {
    match kind {
        NodeKind::File => {
            if let Some(rest) = old_id.strip_prefix("file:") {
                format!("file:{prefix}/{rest}")
            } else {
                format!("file:{prefix}/{old_id}")
            }
        }
        NodeKind::Symbol => {
            if let Some(rest) = old_id.strip_prefix("sym:") {
                format!("sym:{prefix}/{rest}")
            } else {
                format!("sym:{prefix}/{old_id}")
            }
        }
        NodeKind::Semantic => {
            if let Some(rest) = old_id.strip_prefix("sem:") {
                format!("sem:{prefix}/{rest}")
            } else {
                format!("sem:{prefix}/{old_id}")
            }
        }
    }
}

fn link_cross_repo_symbols(merged: &mut CodeGraph) {
    let mut cross_edges = Vec::new();
    for edge in &merged.edges {
        if edge.resolved {
            continue;
        }
        if let Some(candidates) = merged.symbol_index.get(&edge.raw_target)
            && let Some(target_id) = candidates.first()
        {
            cross_edges.push(GraphEdge {
                source: edge.source.clone(),
                target: target_id.clone(),
                kind: GraphEdgeKind::SemanticLink,
                line: edge.line,
                resolved: true,
                raw_target: edge.raw_target.clone(),
            });
        }
    }
    merged.edges.extend(cross_edges);
}
