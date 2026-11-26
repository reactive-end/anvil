//! Angular `@Component` selector-to-template and template-to-component resolution.

use anvil_core::traits::{EdgeKind, EdgeRef};

use crate::model::{CodeGraph, NodeId};
use crate::resolvers::{normalize_join_rel, pick_preferred_symbol};

/// Resolves Angular template, selector, property binding, and event call edges.
pub fn resolve_angular_edge(
    edge: &EdgeRef,
    file_imports: &[String],
    graph: &CodeGraph,
) -> Option<NodeId> {
    match edge.kind {
        EdgeKind::RendersTemplate => resolve_render_target(edge, graph),
        EdgeKind::UsesProp => {
            let selector = edge.target_path_or_symbol.split('.').next().unwrap_or("");
            graph.angular_selectors.get(selector).copied()
        }
        EdgeKind::Calls => resolve_html_call(edge, file_imports, graph),
        _ => None,
    }
}

fn resolve_render_target(edge: &EdgeRef, graph: &CodeGraph) -> Option<NodeId> {
    let target = edge.target_path_or_symbol.trim();
    if target.ends_with(".html") || target.ends_with(".htm") || target.starts_with('.') {
        let base_dir = edge.source_file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let resolved = normalize_join_rel(base_dir, target);
        if graph.file_hashes.contains_key(&resolved) {
            return graph.node_by_id(&CodeGraph::file_node_id(&resolved)).map(|(id, _)| id);
        }
    }
    graph.angular_selectors.get(target).copied()
}

fn resolve_html_call(edge: &EdgeRef, file_imports: &[String], graph: &CodeGraph) -> Option<NodeId> {
    let clean =
        edge.target_path_or_symbol.strip_prefix("this.").unwrap_or(&edge.target_path_or_symbol);
    let method = clean.rsplit('.').next().unwrap_or(clean);

    if let Some(ts_file) = companion_ts_file(&edge.source_file)
        && graph.file_hashes.contains_key(&ts_file)
    {
        let mut candidates = Vec::with_capacity(file_imports.len() + 1);
        candidates.push(ts_file);
        candidates.extend(file_imports.iter().cloned());
        return pick_preferred_symbol(method, &edge.source_file, &candidates, graph);
    }
    pick_preferred_symbol(method, &edge.source_file, file_imports, graph)
}

fn companion_ts_file(html_path: &str) -> Option<String> {
    html_path
        .strip_suffix(".html")
        .or_else(|| html_path.strip_suffix(".htm"))
        .map(|stem| format!("{stem}.ts"))
}
