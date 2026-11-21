//! NetworkX node-link JSON export helper for `store`.

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::model::{CodeGraph, GraphEdgeKind, NodeKind};

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

    let nodes = collect_nx_nodes(graph);
    let links = collect_nx_links(graph);
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

fn collect_nx_nodes(graph: &CodeGraph) -> Vec<NetworkXNode<'_>> {
    let mut nodes: Vec<NetworkXNode<'_>> = graph
        .nodes
        .iter()
        .filter(|n| !n.is_tombstone())
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
    nodes
}

fn collect_nx_links(graph: &CodeGraph) -> Vec<NetworkXLink<'_>> {
    graph
        .edges
        .iter()
        .filter(|e| !e.is_tombstone())
        .filter_map(|e| {
            let src = graph.nodes.get(e.source as usize).filter(|n| !n.is_tombstone())?;
            let (target, raw_target, resolved) = if e.is_resolved() {
                let tgt = graph.nodes.get(e.target as usize).filter(|n| !n.is_tombstone())?;
                (tgt.id.as_str(), tgt.label.as_str(), true)
            } else {
                let ext = graph.external_str(e.target)?;
                (ext, ext, false)
            };
            Some(NetworkXLink {
                source: &src.id,
                target,
                relation: e.kind,
                source_location: format!("L{}", e.line.max(1)),
                resolved,
                raw_target,
                weight: 1.0,
            })
        })
        .collect()
}
