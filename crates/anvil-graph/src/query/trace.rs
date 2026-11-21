//! Shortest path, node explanation, and anti-slop upstream origin tracing.

use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;

use crate::analytics::find_god_nodes;
use crate::model::{CodeGraph, GraphEdgeKind, GraphNode, NodeKind};
use crate::query::resolve_target_node;

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

/// Upstream source-of-truth trace for anti-slop edits (`find_origin`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OriginResult {
    pub queried: String,
    pub origin_node_id: String,
    pub origin_file: String,
    pub origin_label: String,
    pub origin_line: u32,
    pub hops: Vec<String>,
}

/// Finds the shortest directed path between `from` and `to` (resolved by ID, file path, or symbol).
pub fn shortest_path(graph: &CodeGraph, from: &str, to: &str) -> Option<PathResult> {
    let start_id = resolve_target_node(graph, from)?;
    let goal_id = resolve_target_node(graph, to)?;
    if start_id == goal_id {
        return Some(PathResult {
            from: start_id,
            to: goal_id,
            steps: Vec::new(),
        });
    }

    let mut queue = VecDeque::new();
    let mut visited: FxHashSet<String> = FxHashSet::default();
    let mut parent: FxHashMap<String, (String, GraphEdgeKind, u32)> = FxHashMap::default();
    queue.push_back(start_id.clone());
    visited.insert(start_id.clone());

    while let Some(curr) = queue.pop_front() {
        if let Some(edge_idxs) = graph.forward_adj.get(&curr) {
            for &idx in edge_idxs {
                let Some(edge) = graph.edges.get(idx) else {
                    continue;
                };
                if edge.source.is_empty() || !edge.resolved {
                    continue;
                }
                if visited.insert(edge.target.clone()) {
                    parent.insert(edge.target.clone(), (curr.clone(), edge.kind, edge.line));
                    if edge.target == goal_id {
                        return Some(reconstruct_path(start_id, goal_id, &parent));
                    }
                    queue.push_back(edge.target.clone());
                }
            }
        }
    }
    None
}

fn reconstruct_path(
    start_id: String,
    goal_id: String,
    parent: &FxHashMap<String, (String, GraphEdgeKind, u32)>,
) -> PathResult {
    let mut steps = Vec::new();
    let mut cur = goal_id.clone();
    while cur != start_id {
        let Some((prev, kind, line)) = parent.get(&cur) else {
            break;
        };
        steps.push(PathStep {
            from: prev.clone(),
            to: cur.clone(),
            kind: *kind,
            line: *line,
        });
        cur = prev.clone();
    }
    steps.reverse();
    PathResult {
        from: start_id,
        to: goal_id,
        steps,
    }
}

/// Returns incoming/outgoing neighbors, community, and god-node status for `symbol_or_file`.
pub fn explain(graph: &CodeGraph, symbol_or_file: &str) -> Option<ExplainResult> {
    let node_id = resolve_target_node(graph, symbol_or_file)?;
    let node = graph.nodes.get(&node_id)?.clone();

    let incoming = graph
        .incoming_edges(&node_id)
        .into_iter()
        .map(|e| NeighborEdge {
            node_id: e.source.clone(),
            file: graph
                .nodes
                .get(&e.source)
                .map(|n| n.file.clone())
                .unwrap_or_default(),
            kind: e.kind,
            line: e.line,
        })
        .collect();

    let outgoing = graph
        .outgoing_edges(&node_id)
        .into_iter()
        .map(|e| NeighborEdge {
            node_id: e.target.clone(),
            file: graph
                .nodes
                .get(&e.target)
                .map(|n| n.file.clone())
                .unwrap_or_default(),
            kind: e.kind,
            line: e.line,
        })
        .collect();

    let is_god_node = find_god_nodes(graph, 10)
        .iter()
        .any(|g| g.id == node_id && g.total_degree >= 5);

    Some(ExplainResult {
        community: node.community,
        node,
        incoming,
        outgoing,
        is_god_node,
    })
}

/// Traces upstream `ReExports` / `Extends` / `Imports` chains to find the source-of-truth definition.
pub fn find_origin(graph: &CodeGraph, symbol_or_file: &str) -> Option<OriginResult> {
    let start_id = resolve_target_node(graph, symbol_or_file)?;
    let mut current = start_id;
    let mut visited = FxHashSet::default();
    let mut hops = Vec::new();

    while visited.insert(current.clone()) {
        hops.push(current.clone());
        let next = graph
            .outgoing_edges(&current)
            .into_iter()
            .find(|e| {
                e.resolved
                    && matches!(
                        e.kind,
                        GraphEdgeKind::ReExports | GraphEdgeKind::Extends | GraphEdgeKind::Implements
                    )
            })
            .or_else(|| {
                let node = graph.nodes.get(&current)?;
                if node.kind == NodeKind::File && is_barrel_like(&node.file) {
                    graph
                        .outgoing_edges(&current)
                        .into_iter()
                        .find(|e| e.resolved && e.kind == GraphEdgeKind::Imports)
                } else {
                    None
                }
            });

        match next {
            Some(edge) => current = edge.target.clone(),
            None => break,
        }
    }

    let origin = graph.nodes.get(&current)?;
    Some(OriginResult {
        queried: symbol_or_file.to_owned(),
        origin_node_id: origin.id.clone(),
        origin_file: origin.file.clone(),
        origin_label: origin.label.clone(),
        origin_line: origin.line,
        hops,
    })
}

fn is_barrel_like(file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    matches!(
        name,
        "index.ts" | "index.tsx" | "index.js" | "index.jsx" | "__init__.py"
    )
}
