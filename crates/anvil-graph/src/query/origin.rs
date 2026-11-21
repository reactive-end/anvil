//! Anti-slop upstream origin tracing across re-exports, inheritance, and barrels.

use std::collections::VecDeque;

use rustc_hash::FxHashSet;
use serde::Serialize;

use crate::model::{CodeGraph, CompactEdge, GraphEdgeKind, GraphNode, NodeId, NodeKind};
use crate::query::resolve_target_node;

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

/// Traces upstream `ReExports` / `Extends` / `Imports` chains to find the source-of-truth definition.
pub fn find_origin(graph: &CodeGraph, symbol_or_file: &str) -> Option<OriginResult> {
    let (start_id, mut wanted_sym) = resolve_origin_start(graph, symbol_or_file)?;
    if wanted_sym.is_none()
        && let Some(node) = graph.get_node(start_id)
        && node.kind == NodeKind::Symbol
    {
        wanted_sym = Some(node.label.clone());
    }

    let mut current = start_id;
    let mut visited = FxHashSet::default();
    let mut hops = Vec::new();

    while visited.insert(current) {
        let Some(cur_node) = graph.get_node(current) else {
            break;
        };
        hops.push(cur_node.id.clone());

        let next = pick_origin_next_edge(graph, current, cur_node, wanted_sym.as_deref());
        match next {
            Some(edge) => current = edge.target,
            None => break,
        }
    }

    if let Some(sym) = &wanted_sym
        && let Some(cur_node) = graph.get_node(current)
        && cur_node.kind == NodeKind::File
        && let Some(sym_nid) = find_symbol_in_file(graph, &cur_node.file, sym)
    {
        current = sym_nid;
    }

    let origin = graph.get_node(current)?;
    Some(OriginResult {
        queried: symbol_or_file.to_owned(),
        origin_node_id: origin.id.clone(),
        origin_file: origin.file.clone(),
        origin_label: origin.label.clone(),
        origin_line: origin.line,
        hops,
    })
}

fn resolve_origin_start(graph: &CodeGraph, query_str: &str) -> Option<(NodeId, Option<String>)> {
    if let Some(nid) = resolve_target_node(graph, query_str) {
        return Some((nid, None));
    }
    let (file_part, sym_part) = query_str
        .rsplit_once("::")
        .or_else(|| query_str.rsplit_once('#'))?;
    let file_nid = resolve_target_node(graph, file_part)?;
    let sym = sym_part.trim();
    Some((file_nid, (!sym.is_empty()).then(|| sym.to_owned())))
}

fn pick_origin_next_edge<'a>(
    graph: &'a CodeGraph,
    current: NodeId,
    cur_node: &GraphNode,
    wanted_sym: Option<&str>,
) -> Option<&'a CompactEdge> {
    let outgoing = graph.outgoing_edges(current);
    let primary: Vec<&CompactEdge> = outgoing
        .iter()
        .copied()
        .filter(|e| {
            e.is_resolved()
                && matches!(
                    e.kind,
                    GraphEdgeKind::ReExports | GraphEdgeKind::Extends | GraphEdgeKind::Implements
                )
        })
        .collect();

    if !primary.is_empty() {
        if let Some(sym) = wanted_sym
            && let Some(matched) = primary
                .iter()
                .copied()
                .find(|e| target_provides_symbol(graph, e.target, sym, 6))
        {
            return Some(matched);
        }
        return Some(primary[0]);
    }

    if cur_node.kind == NodeKind::File && is_barrel_like(&cur_node.file) {
        let imports: Vec<&CompactEdge> = outgoing
            .into_iter()
            .filter(|e| e.is_resolved() && e.kind == GraphEdgeKind::Imports)
            .collect();
        if let Some(sym) = wanted_sym
            && let Some(matched) = imports
                .iter()
                .copied()
                .find(|e| target_provides_symbol(graph, e.target, sym, 6))
        {
            return Some(matched);
        }
        return imports.into_iter().next();
    }
    None
}

fn target_provides_symbol(
    graph: &CodeGraph,
    start_nid: NodeId,
    sym: &str,
    max_depth: usize,
) -> bool {
    let mut queue = VecDeque::from([(start_nid, 0usize)]);
    let mut seen = FxHashSet::default();
    seen.insert(start_nid);

    while let Some((nid, depth)) = queue.pop_front() {
        let Some(node) = graph.get_node(nid) else {
            continue;
        };
        if node.kind == NodeKind::Symbol && node.label == sym {
            return true;
        }
        if node.kind == NodeKind::File && find_symbol_in_file(graph, &node.file, sym).is_some() {
            return true;
        }
        if depth >= max_depth {
            continue;
        }
        for edge in graph.outgoing_edges(nid) {
            if edge.is_resolved()
                && matches!(edge.kind, GraphEdgeKind::ReExports | GraphEdgeKind::Imports)
                && seen.insert(edge.target)
            {
                queue.push_back((edge.target, depth + 1));
            }
        }
    }
    false
}

fn find_symbol_in_file(graph: &CodeGraph, file: &str, sym: &str) -> Option<NodeId> {
    let nids = graph.file_nodes.get(file)?;
    nids.iter().copied().find(|&nid| {
        graph
            .get_node(nid)
            .is_some_and(|n| n.kind == NodeKind::Symbol && n.label == sym)
    })
}

fn is_barrel_like(file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    matches!(
        name,
        "index.ts" | "index.tsx" | "index.js" | "index.jsx" | "index.mts" | "index.mjs" | "__init__.py"
    )
}
