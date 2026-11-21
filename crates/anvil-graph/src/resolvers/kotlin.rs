//! Kotlin `package`/import, `@Composable` call, and Gradle subproject resolution.

use std::fs;
use std::path::Path;

use anvil_core::traits::{EdgeKind, EdgeRef};
use rustc_hash::FxHashMap;

use crate::model::CodeGraph;
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
) -> Option<String> {
    match edge.kind {
        EdgeKind::Imports => resolve_kotlin_import(&edge.target_path_or_symbol, graph),
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

fn resolve_kotlin_import(target: &str, graph: &CodeGraph) -> Option<String> {
    let trimmed = target.trim();
    if let Some(pkg) = trimmed.strip_suffix(".*") {
        return graph
            .kotlin_packages
            .get(pkg)
            .and_then(|files| files.first())
            .map(|first| CodeGraph::file_node_id(first));
    }

    let (pkg, sym) = trimmed.rsplit_once('.')?;
    if let Some(pkg_files) = graph.kotlin_packages.get(pkg) {
        if let Some(sym_node) = pick_preferred_symbol(sym, "", pkg_files, graph) {
            return Some(sym_node);
        }
        if let Some(first) = pkg_files.first() {
            return Some(CodeGraph::file_node_id(first));
        }
    }

    let suffix = format!("{}/{sym}.kt", pkg.replace('.', "/"));
    graph
        .file_hashes
        .keys()
        .find(|f| f.ends_with(&suffix))
        .map(|f| CodeGraph::file_node_id(f))
}
