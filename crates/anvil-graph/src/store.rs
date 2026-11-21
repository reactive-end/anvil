//! Binary persistence (`postcard`) and NetworkX node-link JSON export.

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::model::{CodeGraph, GraphEdgeKind, NodeKind};

/// Returns the default binary graph path `.harness/.state/graph.bin`.
pub fn default_bin_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".harness").join(".state").join("graph.bin")
}

/// Returns the default JSON export path `.harness/.state/graph.json`.
pub fn default_json_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".harness").join(".state").join("graph.json")
}

/// Serializes `graph` into a compact binary file using an atomic temporary-file swap.
pub fn save_binary(graph: &CodeGraph, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    let bytes = postcard::to_stdvec(graph).context("failed to serialize code graph to postcard")?;
    let tmp_name = format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("graph.bin"),
        std::process::id()
    );
    let tmp_path = path.with_file_name(tmp_name);
    fs::write(&tmp_path, &bytes)
        .with_context(|| format!("failed to write temporary graph at {}", tmp_path.display()))?;
    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(path);
        fs::rename(&tmp_path, path).with_context(|| {
            format!(
                "failed to atomically replace {} (initial error: {err})",
                path.display()
            )
        })?;
    }
    Ok(())
}

/// Loads a `CodeGraph` from `postcard` binary storage and rebuilds fast adjacency indexes.
pub fn load_binary(path: &Path) -> Result<CodeGraph> {
    let bytes =
        fs::read(path).with_context(|| format!("failed to read binary graph {}", path.display()))?;
    let mut graph: CodeGraph =
        postcard::from_bytes(&bytes).context("failed to deserialize binary code graph")?;
    graph.rebuild_indexes();
    Ok(graph)
}

#[derive(Serialize)]
struct NetworkXGraph<'a> {
    directed: bool,
    multigraph: bool,
    graph: NetworkXMeta<'a>,
    nodes: Vec<NetworkXNode<'a>>,
    links: Vec<NetworkXLink<'a>>,
}

#[derive(Serialize)]
struct NetworkXMeta<'a> {
    repo_root: &'a str,
    node_count: usize,
    edge_count: usize,
}

#[derive(Serialize)]
struct NetworkXNode<'a> {
    id: &'a str,
    label: &'a str,
    kind: NodeKind,
    file_type: &'static str,
    source_file: &'a str,
    source_location: String,
    exported: bool,
    community: Option<usize>,
}

#[derive(Serialize)]
struct NetworkXLink<'a> {
    source: &'a str,
    target: &'a str,
    relation: GraphEdgeKind,
    source_location: String,
    resolved: bool,
    raw_target: &'a str,
    weight: f64,
}

/// Exports `graph` in NetworkX node-link JSON format (`nodes` + `links`).
pub fn export_networkx_json(graph: &CodeGraph, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    let mut nodes: Vec<NetworkXNode<'_>> = graph
        .nodes
        .values()
        .map(|n| NetworkXNode {
            id: &n.id,
            label: &n.label,
            kind: n.kind,
            file_type: match n.kind {
                NodeKind::Semantic => "semantic",
                NodeKind::File | NodeKind::Symbol => "code",
            },
            source_file: &n.file,
            source_location: format!("L{}", n.line.max(1)),
            exported: n.exported,
            community: n.community,
        })
        .collect();
    nodes.sort_by(|a, b| a.id.cmp(b.id));

    let links: Vec<NetworkXLink<'_>> = graph
        .edges
        .iter()
        .filter(|e| !e.source.is_empty())
        .map(|e| NetworkXLink {
            source: &e.source,
            target: &e.target,
            relation: e.kind,
            source_location: format!("L{}", e.line.max(1)),
            resolved: e.resolved,
            raw_target: &e.raw_target,
            weight: 1.0,
        })
        .collect();

    let payload = NetworkXGraph {
        directed: true,
        multigraph: false,
        graph: NetworkXMeta {
            repo_root: &graph.repo_root,
            node_count: nodes.len(),
            edge_count: links.len(),
        },
        nodes,
        links,
    };

    let file =
        File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    let writer = BufWriter::new(file);
    serde_json::to_writer_pretty(writer, &payload)
        .with_context(|| format!("failed to write NetworkX JSON to {}", path.display()))?;
    Ok(())
}
