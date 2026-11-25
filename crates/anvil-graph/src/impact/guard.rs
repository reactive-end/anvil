//! Anti-overengineering heuristic flagging new abstractions with <= 1 consumer.

use rustc_hash::FxHashSet;
use serde::Serialize;

use crate::impact::sibling::is_test_file;
use crate::model::{CodeGraph, GraphEdgeKind, NodeId, NodeKind};

/// Warning emitted when a small fix introduces a new abstraction with <= 1 consumer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverengineeringWarning {
    pub file: String,
    pub symbol: Option<String>,
    pub consumer_count: usize,
    pub reason: String,
}

/// Report from `check_overengineering`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverengineeringReport {
    pub flagged: bool,
    pub warnings: Vec<OverengineeringWarning>,
}

/// Flags newly added files or symbols with <= 1 external consumer, especially on small fixes.
pub fn check_overengineering(
    graph: &CodeGraph,
    changed_files: &[String],
    added_files: &[String],
    is_small_fix: bool,
) -> OverengineeringReport {
    let added_norm: FxHashSet<String> = added_files
        .iter()
        .map(|f| CodeGraph::normalize_rel_path(f))
        .collect();
    let mut warnings = Vec::new();
    for norm in &added_norm {
        if is_test_file(norm) || !is_indexed_code_file(graph, norm) {
            continue;
        }
        let consumers = count_external_consumers(graph, norm);
        if consumers <= 1 {
            warnings.push(OverengineeringWarning {
                file: norm.clone(),
                symbol: None,
                consumer_count: consumers,
                reason: if is_small_fix {
                    "Small fix introduced a new file with <= 1 consumer".to_owned()
                } else {
                    "New abstraction file has <= 1 consumer".to_owned()
                },
            });
        }
    }

    if is_small_fix {
        for changed in changed_files {
            let norm = CodeGraph::normalize_rel_path(changed);
            if is_test_file(&norm) || !added_norm.contains(&norm) {
                continue;
            }
            let Some(nids) = graph.file_nodes.get(&norm) else {
                continue;
            };
            for &nid in nids {
                let Some(node) = graph.get_node(nid) else {
                    continue;
                };
                if node.kind != NodeKind::Symbol || !node.exported {
                    continue;
                }
                if count_symbol_external_consumers(graph, nid, &norm) == 0 {
                    warnings.push(OverengineeringWarning {
                        file: norm.clone(),
                        symbol: Some(node.label.clone()),
                        consumer_count: 0,
                        reason: "Newly added exported abstraction has 0 consumers".to_owned(),
                    });
                }
            }
        }
    }

    OverengineeringReport {
        flagged: !warnings.is_empty(),
        warnings,
    }
}

fn is_indexed_code_file(graph: &CodeGraph, file: &str) -> bool {
    if file.starts_with(".harness/")
        || file.starts_with(".agents/")
        || file.starts_with(".claude/")
    {
        return false;
    }
    graph.file_nodes.contains_key(file)
}

fn count_external_consumers(graph: &CodeGraph, file: &str) -> usize {
    let mut callers = FxHashSet::default();
    if let Some(nids) = graph.file_nodes.get(file) {
        for &nid in nids {
            for edge in graph.incoming_edges(nid) {
                if edge.is_resolved()
                    && edge.kind != GraphEdgeKind::Contains
                    && let Some(src) = graph.get_node(edge.source)
                    && src.file != file
                {
                    callers.insert(src.file.as_str());
                }
            }
        }
    }
    callers.len()
}

fn count_symbol_external_consumers(graph: &CodeGraph, sym_id: NodeId, owner_file: &str) -> usize {
    graph
        .incoming_edges(sym_id)
        .into_iter()
        .filter(|e| {
            e.is_resolved()
                && e.kind != GraphEdgeKind::Contains
                && graph
                    .get_node(e.source)
                    .is_some_and(|n| n.file != owner_file)
        })
        .count()
}
