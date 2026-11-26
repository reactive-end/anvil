//! Dart / Flutter `package:` and relative `.dart` import resolution.

use std::fs;
use std::path::Path;

use anvil_core::traits::{EdgeKind, EdgeRef};
use rustc_hash::FxHashMap;

use crate::model::{CodeGraph, NodeId};
use crate::resolvers::{join_normalized, normalize_join_rel, pick_preferred_symbol};

/// Discovers `pubspec.yaml` package names and maps them to their `lib` directory paths.
pub fn load_dart_packages(repo_root: &Path) -> FxHashMap<String, String> {
    let mut packages = FxHashMap::default();
    inspect_pubspec(repo_root, "", &mut packages);
    if let Ok(entries) = fs::read_dir(repo_root) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir()
                && let Some(name) = p.file_name().and_then(|n| n.to_str())
                && !name.starts_with('.')
            {
                inspect_pubspec(&p, name, &mut packages);
            }
        }
    }
    packages
}

fn inspect_pubspec(dir: &Path, rel_dir: &str, out: &mut FxHashMap<String, String>) {
    let pubspec = dir.join("pubspec.yaml");
    let Ok(content) = fs::read_to_string(pubspec) else {
        return;
    };
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("name:") {
            let pkg = rest.trim().trim_matches(|c| c == '\'' || c == '"');
            if !pkg.is_empty() {
                let lib_path =
                    if rel_dir.is_empty() { "lib".to_owned() } else { format!("{rel_dir}/lib") };
                out.insert(pkg.to_owned(), lib_path);
            }
            return;
        }
    }
}

/// Resolves Dart `package:` imports, relative `.dart` imports/exports, and widget/class refs.
pub fn resolve_dart_edge(
    edge: &EdgeRef,
    file_imports: &[String],
    graph: &CodeGraph,
    packages: &FxHashMap<String, String>,
) -> Option<NodeId> {
    match edge.kind {
        EdgeKind::Imports | EdgeKind::ReExports => {
            resolve_dart_uri(&edge.source_file, &edge.target_path_or_symbol, graph, packages)
                .and_then(|rel| graph.node_by_id(&CodeGraph::file_node_id(&rel)).map(|(id, _)| id))
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
            pick_preferred_symbol(sym, &edge.source_file, file_imports, graph)
        }
        _ => None,
    }
}

fn resolve_dart_uri(
    source_file: &str,
    uri: &str,
    graph: &CodeGraph,
    packages: &FxHashMap<String, String>,
) -> Option<String> {
    let trimmed = uri.trim();
    if let Some(pkg_uri) = trimmed.strip_prefix("package:") {
        let (pkg, rest) = pkg_uri.split_once('/')?;
        let lib_root = packages.get(pkg)?;
        let candidate = join_normalized(lib_root, rest);
        return graph.file_hashes.contains_key(&candidate).then_some(candidate);
    }
    if trimmed.starts_with("dart:") {
        return None;
    }

    let base_dir = source_file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let candidate = normalize_join_rel(base_dir, trimmed);
    graph.file_hashes.contains_key(&candidate).then_some(candidate)
}
