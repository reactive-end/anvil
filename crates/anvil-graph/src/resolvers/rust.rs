//! Rust `mod`, `use crate::`, `use super::`, workspace crate, and symbol resolution.

use anvil_core::traits::{EdgeKind, EdgeRef};
use rustc_hash::FxHashMap;

use crate::model::{CodeGraph, NodeId};
use crate::resolvers::{join_normalized, pick_preferred_symbol};

pub use crate::resolvers::rust_cargo::load_rust_crates;

/// Resolves Rust `mod` declarations, `use` paths, trait impls, and calls to `NodeId`s.
pub fn resolve_rust_edge(
    edge: &EdgeRef,
    file_imports: &[String],
    graph: &CodeGraph,
    rust_crates: &FxHashMap<String, String>,
) -> Option<NodeId> {
    match edge.kind {
        EdgeKind::Imports => resolve_rust_import(
            &edge.source_file,
            &edge.target_path_or_symbol,
            graph,
            rust_crates,
        ),
        EdgeKind::Calls | EdgeKind::Implements | EdgeKind::Extends => {
            let raw = edge.target_path_or_symbol.split('<').next().unwrap_or("").trim();
            let sym = raw
                .rsplit("::")
                .next()
                .unwrap_or(raw)
                .rsplit('.')
                .next()
                .unwrap_or(raw)
                .trim();
            pick_preferred_symbol(sym, &edge.source_file, file_imports, graph)
        }
        _ => None,
    }
}

fn resolve_rust_import(
    source_file: &str,
    target: &str,
    graph: &CodeGraph,
    rust_crates: &FxHashMap<String, String>,
) -> Option<NodeId> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.contains("::") {
        return resolve_rust_mod(source_file, trimmed, graph);
    }
    let segs: Vec<&str> = trimmed.split("::").filter(|s| !s.is_empty()).collect();
    if segs.is_empty() {
        return None;
    }

    let resolved_file = match segs[0] {
        "crate" => {
            let src_root = find_crate_src_root(source_file, rust_crates);
            probe_rust_segments(&src_root, &segs[1..], graph)
                .or_else(|| probe_crate_root_fallback(&src_root, &segs[1..], graph))
        }
        "super" => resolve_super_use(source_file, &segs, graph),
        "self" => {
            let sub_dir = current_submodule_dir(source_file);
            probe_rust_segments(&sub_dir, &segs[1..], graph)
        }
        crate_name => {
            let src_root = rust_crates.get(crate_name)?;
            probe_rust_segments(src_root, &segs[1..], graph)
                .or_else(|| probe_crate_root_fallback(src_root, &segs[1..], graph))
        }
    }?;
    graph.node_by_id(&CodeGraph::file_node_id(&resolved_file)).map(|(id, _)| id)
}

fn resolve_rust_mod(source_file: &str, mod_name: &str, graph: &CodeGraph) -> Option<NodeId> {
    let dir = source_file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let fname = source_file.rsplit('/').next().unwrap_or(source_file);
    let mut candidates = Vec::with_capacity(4);
    if !matches!(fname, "mod.rs" | "lib.rs" | "main.rs")
        && let Some(stem) = fname.strip_suffix(".rs")
    {
        let sub = join_normalized(dir, stem);
        candidates.push(join_normalized(&sub, &format!("{mod_name}.rs")));
        candidates.push(join_normalized(&sub, &format!("{mod_name}/mod.rs")));
    }
    candidates.push(join_normalized(dir, &format!("{mod_name}.rs")));
    candidates.push(join_normalized(dir, &format!("{mod_name}/mod.rs")));

    candidates
        .into_iter()
        .find(|c| graph.file_hashes.contains_key(c))
        .and_then(|rel| graph.node_by_id(&CodeGraph::file_node_id(&rel)).map(|(id, _)| id))
}

fn resolve_super_use(source_file: &str, segs: &[&str], graph: &CodeGraph) -> Option<String> {
    let ups = segs.iter().take_while(|&&s| s == "super").count();
    let rest = &segs[ups..];
    let fname = source_file.rsplit('/').next().unwrap_or(source_file);
    let mut dir = source_file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let extra_pops = if matches!(fname, "mod.rs" | "lib.rs" | "main.rs") {
        ups
    } else {
        ups.saturating_sub(1)
    };
    for _ in 0..extra_pops {
        dir = dir.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    }
    probe_rust_segments(dir, rest, graph).or_else(|| {
        if rest.len() == 1 && !dir.is_empty() {
            let direct = format!("{dir}.rs");
            if graph.file_hashes.contains_key(&direct) {
                return Some(direct);
            }
            let mod_rs = format!("{dir}/mod.rs");
            if graph.file_hashes.contains_key(&mod_rs) {
                return Some(mod_rs);
            }
        }
        None
    })
}

fn current_submodule_dir(source_file: &str) -> String {
    let dir = source_file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let fname = source_file.rsplit('/').next().unwrap_or(source_file);
    if matches!(fname, "mod.rs" | "lib.rs" | "main.rs") {
        dir.to_owned()
    } else {
        let stem = fname.strip_suffix(".rs").unwrap_or(fname);
        join_normalized(dir, stem)
    }
}

fn find_crate_src_root(source_file: &str, rust_crates: &FxHashMap<String, String>) -> String {
    let mut best: Option<&str> = None;
    for root in rust_crates.values() {
        let prefix = format!("{root}/");
        if source_file.starts_with(&prefix) && best.is_none_or(|b| root.len() > b.len()) {
            best = Some(root.as_str());
        }
    }
    if let Some(found) = best {
        return found.to_owned();
    }
    if let Some((prefix, _)) = source_file.split_once("/src/") {
        return format!("{prefix}/src");
    }
    "src".to_owned()
}

fn probe_rust_segments(base_dir: &str, segs: &[&str], graph: &CodeGraph) -> Option<String> {
    for len in (1..=segs.len()).rev() {
        let mod_path = segs[..len].join("/");
        let stem = join_normalized(base_dir, &mod_path);
        let rs_file = format!("{stem}.rs");
        if graph.file_hashes.contains_key(&rs_file) {
            return Some(rs_file);
        }
        let mod_rs = format!("{stem}/mod.rs");
        if graph.file_hashes.contains_key(&mod_rs) {
            return Some(mod_rs);
        }
    }
    None
}

fn probe_crate_root_fallback(base_dir: &str, segs: &[&str], graph: &CodeGraph) -> Option<String> {
    if segs.len() != 1 {
        return None;
    }
    for root_file in ["lib.rs", "main.rs"] {
        let candidate = join_normalized(base_dir, root_file);
        if graph.file_hashes.contains_key(&candidate) {
            return Some(candidate);
        }
    }
    None
}
