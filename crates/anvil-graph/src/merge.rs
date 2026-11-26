//! Multi-repo unified graph merging (e.g. `fliiper/` + `centinela/`).

use rustc_hash::FxHashMap;

use crate::model::{
    CodeGraph, CompactEdge, GraphEdgeKind, GraphNode, INVALID_NODE_ID, NodeId, NodeKind,
};
use crate::resolvers::http_routes;

const COMMON_UNRESOLVED_CALLS: &[&str] = &[
    "map",
    "get",
    "set",
    "push",
    "pop",
    "filter",
    "reduce",
    "forEach",
    "find",
    "some",
    "every",
    "len",
    "is_empty",
    "new",
    "from",
    "into",
    "clone",
    "default",
    "to_string",
    "toString",
    "valueOf",
    "trim",
    "split",
    "join",
    "slice",
    "includes",
    "contains",
    "keys",
    "values",
    "entries",
    "parse",
    "stringify",
    "resolve",
    "reject",
    "then",
    "catch",
    "finally",
    "next",
    "iter",
    "collect",
    "unwrap",
    "expect",
    "ok",
    "err",
    "run",
    "init",
    "build",
    "load",
    "save",
    "read",
    "write",
    "open",
    "close",
    "log",
    "warn",
    "error",
    "debug",
    "info",
    "format",
    "print",
    "println",
    "assert",
    "add",
    "remove",
    "insert",
    "delete",
    "clear",
    "update",
    "create",
    "start",
    "stop",
    "handle",
    "apply",
    "call",
    "bind",
    "subscribe",
    "pipe",
    "emit",
    "send",
    "fetch",
    "post",
    "put",
    "patch",
    "head",
];

/// Merges multiple named repository graphs into a single prefix-namespaced `CodeGraph`.
pub fn merge_graphs(repos: &[(&str, &CodeGraph)]) -> CodeGraph {
    let mut merged = CodeGraph::new("multi-repo");
    let mut shared_routes: FxHashMap<String, NodeId> = FxHashMap::default();
    for &(repo_name, sub) in repos {
        merge_subrepo(&mut merged, repo_name, sub, &mut shared_routes);
    }
    merged.rebuild_indexes();
    link_cross_repo_symbols(&mut merged);
    http_routes::link_external_route_calls(&mut merged, false);
    merged.rebuild_adjacency();
    merged
}

fn merge_subrepo(
    merged: &mut CodeGraph,
    repo_name: &str,
    sub: &CodeGraph,
    shared_routes: &mut FxHashMap<String, NodeId>,
) {
    let prefix = CodeGraph::normalize_rel_path(repo_name).trim_matches('/').to_owned();
    let mut id_map = vec![INVALID_NODE_ID; sub.nodes.len()];

    for (rel_path, hash) in &sub.file_hashes {
        merged.file_hashes.insert(format!("{prefix}/{rel_path}"), hash.clone());
    }
    for (rel_path, stat) in &sub.file_stats {
        merged.file_stats.insert(format!("{prefix}/{rel_path}"), *stat);
    }

    for (old_idx, node) in sub.nodes.iter().enumerate() {
        if node.is_tombstone() {
            continue;
        }
        let new_id = prefix_node_id(&prefix, &node.id, node.kind);
        if node.kind == NodeKind::Semantic
            && new_id.starts_with("route:")
            && let Some(&existing) = shared_routes.get(&new_id)
        {
            id_map[old_idx] = existing;
            continue;
        }
        let new_file =
            if node.file.is_empty() { String::new() } else { format!("{prefix}/{}", node.file) };
        let new_nid = merged.nodes.len() as u32;
        if node.kind == NodeKind::Semantic && new_id.starts_with("route:") {
            shared_routes.insert(new_id.clone(), new_nid);
        }
        id_map[old_idx] = new_nid;
        merged.nodes.push(GraphNode {
            id: new_id,
            kind: node.kind,
            label: node.label.clone(),
            file: new_file,
            line: node.line,
            end_line: node.end_line,
            language: node.language,
            symbol_kind: node.symbol_kind,
            signature: node.signature.clone(),
            exported: node.exported,
            community: node.community,
        });
    }

    for edge in &sub.edges {
        if let Some(mapped) = remap_subrepo_edge(merged, sub, edge, &id_map) {
            merged.edges.push(mapped);
        }
    }
}

fn remap_subrepo_edge(
    merged: &mut CodeGraph,
    sub: &CodeGraph,
    edge: &CompactEdge,
    id_map: &[u32],
) -> Option<CompactEdge> {
    if edge.is_tombstone() {
        return None;
    }
    let new_src = id_map.get(edge.source as usize).copied().unwrap_or(INVALID_NODE_ID);
    if new_src == INVALID_NODE_ID {
        return None;
    }
    let new_tgt = if edge.is_resolved() {
        let mapped = id_map.get(edge.target as usize).copied().unwrap_or(INVALID_NODE_ID);
        if mapped == INVALID_NODE_ID {
            return None;
        }
        mapped
    } else {
        let ext = sub.external_str(edge.target)?;
        merged.intern_external(ext)
    };
    Some(CompactEdge { source: new_src, target: new_tgt, kind: edge.kind, line: edge.line })
}

fn prefix_node_id(prefix: &str, old_id: &str, kind: NodeKind) -> String {
    match kind {
        NodeKind::File => {
            format!("file:{prefix}/{}", old_id.strip_prefix("file:").unwrap_or(old_id))
        }
        NodeKind::Symbol => {
            format!("sym:{prefix}/{}", old_id.strip_prefix("sym:").unwrap_or(old_id))
        }
        NodeKind::Semantic => {
            if old_id.starts_with("route:") {
                old_id.to_owned()
            } else {
                format!("sem:{prefix}/{}", old_id.strip_prefix("sem:").unwrap_or(old_id))
            }
        }
    }
}

fn link_cross_repo_symbols(merged: &mut CodeGraph) {
    let mut cross_edges = Vec::new();
    for edge in &merged.edges {
        if edge.is_tombstone() || edge.is_resolved() {
            continue;
        }
        if !matches!(
            edge.kind,
            GraphEdgeKind::Calls
                | GraphEdgeKind::Extends
                | GraphEdgeKind::Implements
                | GraphEdgeKind::Provides
                | GraphEdgeKind::Injects
                | GraphEdgeKind::UsesProp
        ) {
            continue;
        }
        let Some(raw_target) = merged.external_str(edge.target) else {
            continue;
        };
        if !is_linkable_cross_symbol(raw_target) {
            continue;
        }
        let Some(src_node) = merged.get_node(edge.source) else {
            continue;
        };
        let src_repo = src_node.file.split('/').next().unwrap_or("");
        if let Some(target_nid) = find_cross_repo_target(merged, raw_target, src_repo) {
            cross_edges.push(CompactEdge {
                source: edge.source,
                target: target_nid,
                kind: GraphEdgeKind::SemanticLink,
                line: edge.line,
            });
        }
    }
    merged.edges.extend(cross_edges);
}

fn is_linkable_cross_symbol(raw: &str) -> bool {
    let t = raw.trim();
    t.len() >= 4 && !t.contains('.') && !t.contains('/') && !COMMON_UNRESOLVED_CALLS.contains(&t)
}

fn find_cross_repo_target(merged: &CodeGraph, raw_target: &str, src_repo: &str) -> Option<NodeId> {
    let candidates = merged.symbol_index.get(raw_target)?;
    let matches: Vec<NodeId> = candidates
        .iter()
        .copied()
        .filter(|&nid| {
            merged.get_node(nid).is_some_and(|n| {
                n.label == raw_target
                    && n.exported
                    && n.file.split('/').next().unwrap_or("") != src_repo
            })
        })
        .collect();
    (!matches.is_empty() && matches.len() <= 3).then(|| matches[0])
}
