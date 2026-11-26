//! Python relative (`.`, `..`) and package module import resolution.

use anvil_core::traits::{EdgeKind, EdgeRef};

use crate::model::{CodeGraph, NodeId};
use crate::resolvers::{join_normalized, pick_preferred_symbol};

/// Resolves Python imports, inheritance, and call targets to internal `NodeId`s.
pub fn resolve_python_edge(
    edge: &EdgeRef,
    file_imports: &[String],
    graph: &CodeGraph,
) -> Option<NodeId> {
    match edge.kind {
        EdgeKind::Imports => {
            resolve_python_import(&edge.source_file, &edge.target_path_or_symbol, graph)
                .and_then(|rel| graph.node_by_id(&CodeGraph::file_node_id(&rel)).map(|(id, _)| id))
        }
        EdgeKind::Calls | EdgeKind::Extends => {
            let sym = edge
                .target_path_or_symbol
                .rsplit('.')
                .next()
                .unwrap_or(&edge.target_path_or_symbol);
            pick_preferred_symbol(sym, &edge.source_file, file_imports, graph)
        }
        _ => None,
    }
}

/// Resolves a Python dotted or relative module specifier to a repository-relative file path.
pub fn resolve_python_import(
    source_file: &str,
    specifier: &str,
    graph: &CodeGraph,
) -> Option<String> {
    let trimmed = specifier.trim();
    if trimmed.is_empty() {
        return None;
    }

    let dots = trimmed.bytes().take_while(|&b| b == b'.').count();
    let rest = &trimmed[dots..];
    let mod_rel = rest.replace('.', "/");

    if dots > 0 {
        let mut dir = source_file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        for _ in 1..dots {
            dir = dir.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        }
        let stem = join_normalized(dir, &mod_rel);
        return probe_python_file(&stem, graph);
    }

    if let Some(found) = probe_python_file(&mod_rel, graph) {
        return Some(found);
    }
    let src_stem = join_normalized("src", &mod_rel);
    probe_python_file(&src_stem, graph)
}

fn probe_python_file(stem: &str, graph: &CodeGraph) -> Option<String> {
    let clean = stem.trim_end_matches('/');
    let candidates =
        [format!("{clean}.py"), format!("{clean}.pyi"), format!("{clean}/__init__.py")];
    candidates.into_iter().find(|c| graph.file_hashes.contains_key(c))
}
