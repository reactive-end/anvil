//! NestJS `@Module` (`imports`, `providers`, `controllers`) and constructor DI resolution.

use anvil_core::traits::{EdgeKind, EdgeRef};

use crate::model::{CodeGraph, NodeId};
use crate::resolvers::pick_preferred_symbol;

/// Resolves NestJS module composition and dependency-injection symbol edges.
pub fn resolve_nestjs_edge(
    edge: &EdgeRef,
    file_imports: &[String],
    graph: &CodeGraph,
) -> Option<NodeId> {
    if !matches!(
        edge.kind,
        EdgeKind::Provides | EdgeKind::Injects | EdgeKind::Imports
    ) {
        return None;
    }

    let target = edge.target_path_or_symbol.trim();
    if target.is_empty()
        || target.contains('/')
        || target.starts_with('.')
        || !target.starts_with(char::is_uppercase)
    {
        return None;
    }

    if let Some(sym) = pick_preferred_symbol(target, &edge.source_file, file_imports, graph) {
        return Some(sym);
    }

    let ids = graph.symbol_index.get(target)?;
    let exact: Vec<NodeId> = ids
        .iter()
        .copied()
        .filter(|&id| graph.get_node(id).is_some_and(|n| n.label == target))
        .collect();
    (exact.len() == 1).then_some(exact[0])
}
