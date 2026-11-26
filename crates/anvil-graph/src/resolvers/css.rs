//! CSS and SCSS `@import` relative and `_partial.scss` file resolution.

use anvil_core::traits::{EdgeKind, EdgeRef};

use crate::model::{CodeGraph, NodeId};
use crate::resolvers::normalize_join_rel;

/// Resolves relative CSS/SCSS `@import` targets (including SCSS `_partial` convention).
pub fn resolve_css_import(edge: &EdgeRef, graph: &CodeGraph) -> Option<NodeId> {
    if edge.kind != EdgeKind::Imports {
        return None;
    }
    let raw = edge.target_path_or_symbol.trim();
    if raw.is_empty()
        || raw.starts_with("http://")
        || raw.starts_with("https://")
        || raw.starts_with("//")
        || raw.starts_with("data:")
    {
        return None;
    }
    let clean = raw.split(['?', '#']).next().unwrap_or(raw).trim();
    let base_dir = edge.source_file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let joined = normalize_join_rel(base_dir, clean);

    let (dir, fname) = joined.rsplit_once('/').unwrap_or(("", &joined));
    let mut candidates = vec![
        joined.clone(),
        format!("{joined}.css"),
        format!("{joined}.scss"),
        format!("{joined}.sass"),
        format!("{joined}/index.css"),
        format!("{joined}/index.scss"),
        format!("{joined}/_index.scss"),
    ];
    if !fname.starts_with('_') {
        let partial_base =
            if dir.is_empty() { format!("_{fname}") } else { format!("{dir}/_{fname}") };
        candidates.push(partial_base.clone());
        candidates.push(format!("{partial_base}.scss"));
        candidates.push(format!("{partial_base}.sass"));
        candidates.push(format!("{partial_base}.css"));
    }

    candidates
        .into_iter()
        .find(|c| graph.file_hashes.contains_key(c))
        .and_then(|rel| graph.node_by_id(&CodeGraph::file_node_id(&rel)).map(|(id, _)| id))
}
