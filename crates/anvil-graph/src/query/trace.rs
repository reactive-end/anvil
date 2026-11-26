//! Shortest path and architectural node explanation.

use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;

use crate::model::{CodeGraph, GraphEdgeKind, GraphNode, NodeId};
use crate::query::resolve_target_node;

const GOD_NODE_DEGREE_THRESHOLD: usize = 12;

/// Directed edge step in a shortest path between two graph nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PathStep {
    pub from: String,
    pub to: String,
    pub kind: GraphEdgeKind,
    pub line: u32,
}

/// Shortest-path result between two symbols or files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PathResult {
    pub from: String,
    pub to: String,
    pub steps: Vec<PathStep>,
}

/// Neighbor summary for `explain`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NeighborEdge {
    pub node_id: String,
    pub file: String,
    pub kind: GraphEdgeKind,
    pub line: u32,
}

/// Comprehensive architectural explanation for a file or symbol node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExplainResult {
    pub node: GraphNode,
    pub incoming: Vec<NeighborEdge>,
    pub outgoing: Vec<NeighborEdge>,
    pub community: Option<usize>,
    pub is_god_node: bool,
}

/// Finds the shortest directed path between `from` and `to` (resolved by ID, file path, or symbol).
pub fn shortest_path(graph: &CodeGraph, from: &str, to: &str) -> Option<PathResult> {
    let start_id = resolve_target_node(graph, from)?;
    let goal_id = resolve_target_node(graph, to)?;
    if start_id == goal_id {
        let id_str = graph.get_node(start_id)?.id.clone();
        return Some(PathResult { from: id_str.clone(), to: id_str, steps: Vec::new() });
    }

    let mut queue = VecDeque::new();
    let mut visited: FxHashSet<NodeId> = FxHashSet::default();
    let mut parent: FxHashMap<NodeId, (NodeId, GraphEdgeKind, u32)> = FxHashMap::default();
    queue.push_back(start_id);
    visited.insert(start_id);

    while let Some(curr) = queue.pop_front() {
        let Some(edge_idxs) = graph.forward_adj.get(curr as usize) else {
            continue;
        };
        for &idx in edge_idxs {
            let Some(edge) = graph.edges.get(idx as usize) else {
                continue;
            };
            if !edge.is_resolved() {
                continue;
            }
            if visited.insert(edge.target) {
                parent.insert(edge.target, (curr, edge.kind, edge.line));
                if edge.target == goal_id {
                    return Some(reconstruct_path(graph, start_id, goal_id, &parent));
                }
                queue.push_back(edge.target);
            }
        }
    }
    None
}

fn reconstruct_path(
    graph: &CodeGraph,
    start_id: NodeId,
    goal_id: NodeId,
    parent: &FxHashMap<NodeId, (NodeId, GraphEdgeKind, u32)>,
) -> PathResult {
    let mut steps = Vec::new();
    let mut cur = goal_id;
    while cur != start_id {
        let Some(&(prev, kind, line)) = parent.get(&cur) else {
            break;
        };
        let from_str = graph.get_node(prev).map_or_else(String::new, |n| n.id.clone());
        let to_str = graph.get_node(cur).map_or_else(String::new, |n| n.id.clone());
        steps.push(PathStep { from: from_str, to: to_str, kind, line });
        cur = prev;
    }
    steps.reverse();
    PathResult {
        from: graph.get_node(start_id).map_or_else(String::new, |n| n.id.clone()),
        to: graph.get_node(goal_id).map_or_else(String::new, |n| n.id.clone()),
        steps,
    }
}

/// Returns incoming/outgoing neighbors, community, and O(1) degree god-node status.
pub fn explain(graph: &CodeGraph, symbol_or_file: &str) -> Option<ExplainResult> {
    let nid = resolve_target_node(graph, symbol_or_file)?;
    let node = graph.get_node(nid)?.clone();

    let mut non_contain_in = 0usize;
    let incoming: Vec<NeighborEdge> = graph
        .incoming_edges(nid)
        .into_iter()
        .filter_map(|e| {
            if e.kind != GraphEdgeKind::Contains {
                non_contain_in += 1;
            }
            let src = graph.get_node(e.source)?;
            Some(NeighborEdge {
                node_id: src.id.clone(),
                file: src.file.clone(),
                kind: e.kind,
                line: e.line,
            })
        })
        .collect();

    let mut non_contain_out = 0usize;
    let outgoing: Vec<NeighborEdge> = graph
        .outgoing_edges(nid)
        .into_iter()
        .filter_map(|e| {
            if e.kind != GraphEdgeKind::Contains {
                non_contain_out += 1;
            }
            let (node_id, file) = if e.is_resolved() {
                let tgt = graph.get_node(e.target)?;
                (tgt.id.clone(), tgt.file.clone())
            } else {
                let ext = graph.external_str(e.target)?;
                (format!("ext:{ext}"), String::new())
            };
            Some(NeighborEdge { node_id, file, kind: e.kind, line: e.line })
        })
        .collect();

    let is_god_node = non_contain_in + non_contain_out >= GOD_NODE_DEGREE_THRESHOLD;

    Some(ExplainResult { community: node.community, node, incoming, outgoing, is_god_node })
}
