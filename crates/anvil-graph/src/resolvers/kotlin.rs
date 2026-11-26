//! Kotlin `package`/import, `@Composable` call, and Gradle subproject resolution.

use std::fs;
use std::path::Path;

use anvil_core::traits::{EdgeKind, EdgeRef};
use rustc_hash::FxHashMap;

use crate::model::{CodeGraph, NodeId};
use crate::resolvers::pick_preferred_symbol;

/// Parses `settings.gradle.kts` / `settings.gradle` for `include(":mod")` subprojects.
pub fn load_gradle_subprojects(repo_root: &Path) -> FxHashMap<String, String> {
    let mut map = FxHashMap::default();
    for name in ["settings.gradle.kts", "settings.gradle"] {
        let path = repo_root.join(name);
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        for seg in content.split('"').chain(content.split('\'')) {
            if seg.starts_with(':') && seg.len() > 1 {
                let rel = seg.trim_start_matches(':').replace(':', "/");
                map.insert(seg.to_owned(), rel);
            }
        }
    }
    map
}

/// Extracts `package ...` declaration from Kotlin source text if present in the header.
pub fn extract_kotlin_package(source: &str) -> Option<String> {
    for line in source.lines().take(20) {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("package ") {
            let pkg = rest.trim_end_matches(';').trim();
            if !pkg.is_empty() {
                return Some(pkg.to_owned());
            }
        }
    }
    None
}

/// Resolves Kotlin imports, `@Composable` function calls, and class/interface delegations.
pub fn resolve_kotlin_edge(
    edge: &EdgeRef,
    file_imports: &[String],
    graph: &CodeGraph,
    gradle_subprojects: &FxHashMap<String, String>,
) -> Option<NodeId> {
    match edge.kind {
        EdgeKind::Imports => {
            resolve_kotlin_import(&edge.target_path_or_symbol, graph, gradle_subprojects)
        }
        EdgeKind::Calls | EdgeKind::Extends | EdgeKind::Implements => {
            let sym = edge
                .target_path_or_symbol
                .split('<')
                .next()
                .unwrap_or(&edge.target_path_or_symbol)
                .rsplit('.')
                .next()
                .unwrap_or(&edge.target_path_or_symbol)
                .trim();

            let mut candidates = file_imports.to_vec();
            if let Some(pkg) = graph.file_packages.get(&edge.source_file)
                && let Some(same_pkg_files) = graph.kotlin_packages.get(pkg)
            {
                candidates.extend(same_pkg_files.iter().cloned());
            }
            pick_preferred_symbol(sym, &edge.source_file, &candidates, graph)
        }
        _ => None,
    }
}

fn resolve_kotlin_import(
    target: &str,
    graph: &CodeGraph,
    gradle_subprojects: &FxHashMap<String, String>,
) -> Option<NodeId> {
    let trimmed = target.trim();
    if let Some(pkg) = trimmed.strip_suffix(".*") {
        return graph
            .kotlin_packages
            .get(pkg)
            .and_then(|files| files.first())
            .and_then(|first| graph.node_by_id(&CodeGraph::file_node_id(first)).map(|(id, _)| id));
    }

    let (pkg, sym) = trimmed.rsplit_once('.')?;
    if let Some(pkg_files) = graph.kotlin_packages.get(pkg) {
        if let Some(sym_node) = pick_preferred_symbol(sym, "", pkg_files, graph) {
            return Some(sym_node);
        }
        if let Some(first) = pkg_files.first()
            && let Some((id, _)) = graph.node_by_id(&CodeGraph::file_node_id(first))
        {
            return Some(id);
        }
    }

    let pkg_path = pkg.replace('.', "/");
    if let Some(sub_hit) = probe_gradle_subprojects(graph, gradle_subprojects, &pkg_path, sym) {
        return Some(sub_hit);
    }

    let suffix = format!("{pkg_path}/{sym}.kt");
    graph
        .file_hashes
        .keys()
        .find(|f| f.ends_with(&suffix))
        .and_then(|f| graph.node_by_id(&CodeGraph::file_node_id(f)).map(|(id, _)| id))
}

fn probe_gradle_subprojects(
    graph: &CodeGraph,
    subprojects: &FxHashMap<String, String>,
    pkg_path: &str,
    sym: &str,
) -> Option<NodeId> {
    const SRC_ROOTS: &[&str] =
        &["src/main/kotlin", "src/commonMain/kotlin", "src/main/java", "src"];
    for sub_rel in subprojects.values() {
        for src_root in SRC_ROOTS {
            let candidate = format!("{sub_rel}/{src_root}/{pkg_path}/{sym}.kt");
            if graph.file_hashes.contains_key(&candidate)
                && let Some((id, _)) = graph.node_by_id(&CodeGraph::file_node_id(&candidate))
            {
                return Some(id);
            }
        }
        if let Some(ids) = graph.symbol_index.get(sym) {
            let prefix = format!("{sub_rel}/");
            for &nid in ids {
                if let Some(node) = graph.get_node(nid)
                    && node.label == sym
                    && node.file.starts_with(&prefix)
                {
                    return Some(nid);
                }
            }
        }
    }
    None
}
