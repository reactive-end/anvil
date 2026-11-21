//! Compact `postcard` wire representation and node-ID suffix compression for `store.rs`.

use anvil_core::traits::{Language, SymbolKind};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::model::{CodeGraph, CompactEdge, GraphNode, INVALID_NODE_ID, NodeKind};

#[derive(Serialize, Deserialize)]
pub(super) struct GraphWire {
    pub repo_root: String,
    pub files: Vec<(String, String, (u64, u64))>,
    pub nodes: Vec<WireNode>,
    pub edges: Vec<CompactEdge>,
    pub externals: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct WireNode {
    pub file_idx: u32,
    pub id_suffix: String,
    pub kind: NodeKind,
    pub label: String,
    pub line: u32,
    pub language: Option<Language>,
    pub symbol_kind: Option<SymbolKind>,
    pub signature: Option<String>,
    pub exported: bool,
    pub community: Option<usize>,
}

pub(super) fn build_wire_nodes(
    graph: &CodeGraph,
    file_to_idx: &FxHashMap<&str, u32>,
) -> (Vec<WireNode>, u32) {
    let mut live_nodes = 0u32;
    let nodes = graph
        .nodes
        .iter()
        .map(|n| {
            if !n.is_tombstone() {
                live_nodes = live_nodes.saturating_add(1);
            }
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
    (nodes, live_nodes)
}

pub(super) fn unpack_wire_nodes(wire_nodes: Vec<WireNode>, file_paths: &[String]) -> Vec<GraphNode> {
    wire_nodes
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
        .collect()
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
