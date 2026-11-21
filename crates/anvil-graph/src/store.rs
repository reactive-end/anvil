//! Compact binary persistence (`postcard`) and NetworkX node-link JSON export.

mod export;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use anvil_core::fs_util::atomic_write;
use anvil_core::traits::{Language, SymbolKind};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::model::{CodeGraph, CompactEdge, GraphNode, INVALID_NODE_ID, NodeKind};

pub use export::export_networkx_json;

/// Returns the default binary graph path `.harness/.state/graph.bin`.
pub fn default_bin_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".harness").join(".state").join("graph.bin")
}

/// Returns the default JSON export path `.harness/.state/graph.json`.
pub fn default_json_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".harness").join(".state").join("graph.json")
}

#[derive(Serialize, Deserialize)]
struct GraphWire {
    repo_root: String,
    files: Vec<(String, String, (u64, u64))>,
    nodes: Vec<WireNode>,
    edges: Vec<CompactEdge>,
    externals: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct WireNode {
    file_idx: u32,
    id_suffix: String,
    kind: NodeKind,
    label: String,
    line: u32,
    language: Option<Language>,
    symbol_kind: Option<SymbolKind>,
    signature: Option<String>,
    exported: bool,
    community: Option<usize>,
}

/// Serializes `graph` into a compact deduplicated binary file using `atomic_write`.
pub fn save_binary(graph: &CodeGraph, path: &Path) -> Result<()> {
    let mut files = Vec::with_capacity(graph.file_hashes.len());
    let mut file_to_idx: FxHashMap<&str, u32> =
        FxHashMap::with_capacity_and_hasher(graph.file_hashes.len(), Default::default());
    for (rel, hash) in &graph.file_hashes {
        let stat = graph.file_stats.get(rel).copied().unwrap_or((0, 0));
        let idx = files.len() as u32;
        file_to_idx.insert(rel.as_str(), idx);
        files.push((rel.clone(), hash.clone(), stat));
    }

    let nodes = graph
        .nodes
        .iter()
        .map(|n| {
            let file_idx = file_to_idx.get(n.file.as_str()).copied().unwrap_or(INVALID_NODE_ID);
            let id_suffix = encode_node_id_suffix(n, file_idx);
            let label = if n.kind == NodeKind::File { String::new() } else { n.label.clone() };
            WireNode {
                file_idx,
                id_suffix,
                kind: n.kind,
                label,
                line: n.line,
                language: n.language,
                symbol_kind: n.symbol_kind,
                signature: n.signature.clone(),
                exported: n.exported,
                community: n.community,
            }
        })
        .collect();

    let wire = GraphWire {
        repo_root: graph.repo_root.clone(),
        files,
        nodes,
        edges: graph.edges.clone(),
        externals: graph.externals.clone(),
    };
    let bytes = postcard::to_stdvec(&wire).context("failed to serialize code graph to postcard")?;
    atomic_write(path, &bytes)
}

/// Loads a `CodeGraph` from compact `postcard` binary storage and rebuilds fast indexes.
pub fn load_binary(path: &Path) -> Result<CodeGraph> {
    let bytes =
        fs::read(path).with_context(|| format!("failed to read binary graph {}", path.display()))?;
    let wire: GraphWire =
        postcard::from_bytes(&bytes).context("failed to deserialize binary code graph")?;

    let file_count = wire.files.len();
    let mut file_paths = Vec::with_capacity(file_count);
    let mut file_hashes = FxHashMap::with_capacity_and_hasher(file_count, Default::default());
    let mut file_stats = FxHashMap::with_capacity_and_hasher(file_count, Default::default());
    for (rel, hash, stat) in wire.files {
        file_paths.push(rel.clone());
        file_hashes.insert(rel.clone(), hash);
        if stat != (0, 0) {
            file_stats.insert(rel, stat);
        }
    }

    let nodes = wire
        .nodes
        .into_iter()
        .map(|wn| {
            let file = file_paths.get(wn.file_idx as usize).cloned().unwrap_or_default();
            let label = if wn.kind == NodeKind::File && wn.label.is_empty() {
                file.rsplit('/').next().unwrap_or(&file).to_owned()
            } else {
                wn.label
            };
            let id = decode_node_id(wn.kind, &file, &label, wn.id_suffix);
            GraphNode {
                id,
                kind: wn.kind,
                label,
                file,
                line: wn.line,
                language: wn.language,
                symbol_kind: wn.symbol_kind,
                signature: wn.signature,
                exported: wn.exported,
                community: wn.community,
            }
        })
        .collect();

    let mut graph = CodeGraph {
        repo_root: wire.repo_root,
        file_hashes,
        file_stats,
        nodes,
        edges: wire.edges,
        externals: wire.externals,
        ..CodeGraph::default()
    };
    graph.rebuild_indexes();
    Ok(graph)
}

fn encode_node_id_suffix(node: &GraphNode, file_idx: u32) -> String {
    if node.is_tombstone() {
        return String::new();
    }
    if file_idx != INVALID_NODE_ID {
        match node.kind {
            NodeKind::File => return String::new(),
            NodeKind::Symbol => {
                let prefix = format!("sym:{}::", node.file);
                if let Some(rest) = node.id.strip_prefix(&prefix) {
                    return if rest == node.label { String::new() } else { rest.to_owned() };
                }
            }
            NodeKind::Semantic => {}
        }
    }
    node.id.clone()
}

fn decode_node_id(kind: NodeKind, file: &str, label: &str, suffix: String) -> String {
    match kind {
        NodeKind::File if suffix.is_empty() && !file.is_empty() => CodeGraph::file_node_id(file),
        NodeKind::Symbol if !file.is_empty() && suffix.is_empty() => {
            CodeGraph::symbol_node_id(file, label)
        }
        NodeKind::Symbol if !file.is_empty() && !suffix.starts_with("sym:") => {
            CodeGraph::symbol_node_id(file, &suffix)
        }
        _ => suffix,
    }
}
