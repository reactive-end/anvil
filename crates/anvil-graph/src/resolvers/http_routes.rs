//! Canonical HTTP route node materialization and cross-repo `CallsRoute` resolution.

use anvil_core::traits::{EdgeKind, EdgeRef, Language};
use rustc_hash::FxHashSet;

use crate::model::{CodeGraph, CompactEdge, GraphEdgeKind, GraphNode, NodeId, NodeKind};

/// Splits `"route:<METHOD> <path>"` into `(method, path)`.
pub fn parse_route_key(raw: &str) -> Option<(&str, &str)> {
    let rest = raw.trim().strip_prefix("route:")?;
    let (method, path) = rest.split_once(' ')?;
    (!method.is_empty() && !path.is_empty()).then_some((method, path))
}

/// Returns the human-readable display label for a canonical `"route:<METHOD> <path>"` key.
pub fn route_label(canonical_route: &str) -> &str {
    canonical_route.strip_prefix("route:").unwrap_or(canonical_route)
}

/// Materializes `route:<METHOD> <path>` virtual nodes and `route -> provider_file` edges.
pub(crate) fn materialize_exposed_routes(
    graph: &mut CodeGraph,
    pending: &[(Language, Vec<EdgeRef>)],
    update_adj: bool,
) {
    let mut seen: FxHashSet<(NodeId, NodeId)> = FxHashSet::default();
    for (_, edges) in pending {
        materialize_slice(graph, edges, update_adj, &mut seen);
    }
}

/// Materializes exposed routes for a single file's extracted `EdgeRef`s.
pub(crate) fn materialize_exposed_routes_for_file(
    graph: &mut CodeGraph,
    raw_edges: &[EdgeRef],
    update_adj: bool,
) {
    let mut seen: FxHashSet<(NodeId, NodeId)> = FxHashSet::default();
    materialize_slice(graph, raw_edges, update_adj, &mut seen);
}

fn materialize_slice(
    graph: &mut CodeGraph,
    raw_edges: &[EdgeRef],
    update_adj: bool,
    seen: &mut FxHashSet<(NodeId, NodeId)>,
) {
    for edge in raw_edges {
        if edge.kind != EdgeKind::ExposesRoute {
            continue;
        }
        let route_key = edge.target_path_or_symbol.trim();
        if parse_route_key(route_key).is_none() {
            continue;
        }
        let file_id = CodeGraph::file_node_id(&edge.source_file);
        let Some((file_nid, _)) = graph.node_by_id(&file_id) else {
            continue;
        };
        let route_nid = ensure_route_node(graph, route_key);
        if !seen.insert((route_nid, file_nid)) {
            continue;
        }
        if update_adj && has_provider_edge(graph, route_nid, file_nid) {
            continue;
        }
        graph.push_edge(
            CompactEdge {
                source: route_nid,
                target: file_nid,
                kind: GraphEdgeKind::ExposesRoute,
                line: edge.line,
            },
            update_adj,
        );
    }
}

fn ensure_route_node(graph: &mut CodeGraph, route_key: &str) -> NodeId {
    if let Some((nid, _)) = graph.node_by_id(route_key) {
        return nid;
    }
    graph.insert_node_with_slot(
        GraphNode {
            id: route_key.to_owned(),
            kind: NodeKind::Semantic,
            label: route_label(route_key).to_owned(),
            file: String::new(),
            line: 0,
            end_line: 0,
            language: None,
            symbol_kind: None,
            signature: Some("http_route".to_owned()),
            exported: false,
            community: None,
        },
        None,
    )
}

fn has_provider_edge(graph: &CodeGraph, route_nid: NodeId, file_nid: NodeId) -> bool {
    graph
        .outgoing_edges(route_nid)
        .into_iter()
        .any(|e| e.kind == GraphEdgeKind::ExposesRoute && e.is_resolved() && e.target == file_nid)
}

/// Resolves a `CallsRoute` key (`"route:<METHOD> <path>"` or `"route:ANY <path>"`) to matching nodes.
pub fn resolve_calls_route_targets(raw_target: &str, graph: &CodeGraph) -> Vec<NodeId> {
    let Some((method, path)) = parse_route_key(raw_target) else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    if method == "ANY" {
        for (key, &nid) in &graph.semantic_index {
            if let Some((_, p)) = parse_route_key(key)
                && p == path
                && graph.get_node(nid).is_some()
            {
                targets.push(nid);
            }
        }
        targets.sort_unstable();
        targets.dedup();
        return targets;
    }

    if let Some((nid, _)) = graph.node_by_id(raw_target.trim()) {
        targets.push(nid);
    }
    let any_key = format!("route:ANY {path}");
    if let Some((nid, _)) = graph.node_by_id(&any_key)
        && !targets.contains(&nid)
    {
        targets.push(nid);
    }
    targets
}

/// Links any unresolved external `CallsRoute` edges to matching `route:` nodes in `graph`.
pub(crate) fn link_external_route_calls(graph: &mut CodeGraph, update_adj: bool) {
    let mut updates: Vec<(usize, Vec<NodeId>)> = Vec::new();
    for (idx, edge) in graph.edges.iter().enumerate() {
        if edge.is_tombstone() || edge.is_resolved() || edge.kind != GraphEdgeKind::CallsRoute {
            continue;
        }
        let Some(raw) = graph.external_str(edge.target) else {
            continue;
        };
        let targets = resolve_calls_route_targets(raw, graph);
        if !targets.is_empty() {
            updates.push((idx, targets));
        }
    }

    for (edge_idx, targets) in updates {
        let first = targets[0];
        let (src, line) = {
            let edge = &mut graph.edges[edge_idx];
            edge.target = first;
            (edge.source, edge.line)
        };
        if update_adj {
            let target_slot = first as usize;
            if target_slot >= graph.reverse_adj.len() {
                graph.reverse_adj.resize_with(target_slot + 1, Vec::new);
            }
            graph.reverse_adj[target_slot].push(edge_idx as u32);
        }
        for &extra_tid in &targets[1..] {
            graph.push_edge(
                CompactEdge {
                    source: src,
                    target: extra_tid,
                    kind: GraphEdgeKind::CallsRoute,
                    line,
                },
                update_adj,
            );
        }
    }
}
