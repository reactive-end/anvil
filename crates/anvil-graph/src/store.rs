//! Compact binary persistence (`postcard` with 24-byte stats header) and NetworkX JSON export.

mod export;
mod wire;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anvil_core::fs_util::atomic_write;
use anyhow::{Context, Result, bail};
use rustc_hash::FxHashMap;

use crate::model::CodeGraph;

pub use export::export_networkx_json;
use wire::{GraphWire, build_wire_nodes, unpack_wire_nodes};

const GRAPH_MAGIC: &[u8; 8] = b"ANVLGRPH";
const GRAPH_VERSION: u32 = 3;
const HEADER_LEN: usize = 24;

/// Fixed 24-byte header prepended to `graph.bin` for sub-0.1ms stats reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphHeader {
    pub version: u32,
    pub live_nodes: u32,
    pub live_edges: u32,
    pub file_count: u32,
}

/// Reads ONLY the 24-byte header of `graph.bin` with zero graph deserialization.
pub fn read_header(path: &Path) -> Result<GraphHeader> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("failed to open binary graph {}", path.display()))?;
    let mut buf = [0u8; HEADER_LEN];
    file.read_exact(&mut buf)
        .with_context(|| format!("failed to read header from {}", path.display()))?;
    parse_header(&buf)
}

/// Returns the default binary graph path `.harness/.state/graph.bin`.
pub fn default_bin_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".harness").join(".state").join("graph.bin")
}

/// Returns the default JSON export path `.harness/.state/graph.json`.
pub fn default_json_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".harness").join(".state").join("graph.json")
}

/// Serializes `graph` into a header-prefixed compact binary file using `atomic_write`.
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

    let (nodes, live_nodes) = build_wire_nodes(graph, &file_to_idx);
    let live_edges =
        u32::try_from(graph.edges.iter().filter(|e| !e.is_tombstone()).count()).unwrap_or(u32::MAX);
    let file_count = u32::try_from(files.len()).unwrap_or(u32::MAX);

    let wire = GraphWire {
        repo_root: graph.repo_root.clone(),
        files,
        nodes,
        edges: graph.edges.clone(),
        externals: graph.externals.clone(),
    };
    let header =
        encode_header(GraphHeader { version: GRAPH_VERSION, live_nodes, live_edges, file_count });
    let mut bytes = Vec::with_capacity(HEADER_LEN + graph.nodes.len() * 32 + graph.edges.len() * 8);
    bytes.extend_from_slice(&header);
    postcard::to_io(&wire, &mut bytes).context("failed to serialize code graph to postcard")?;
    atomic_write(path, &bytes)
}

/// Loads a `CodeGraph` from compact `postcard` binary storage and rebuilds fast indexes.
pub fn load_binary(path: &Path) -> Result<CodeGraph> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read binary graph {}", path.display()))?;
    let _header = parse_header(&bytes)?;
    let wire: GraphWire = postcard::from_bytes(&bytes[HEADER_LEN..])
        .context("failed to deserialize binary code graph")?;

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

    let nodes = unpack_wire_nodes(wire.nodes, &file_paths);
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

fn encode_header(header: GraphHeader) -> [u8; HEADER_LEN] {
    let mut buf = [0u8; HEADER_LEN];
    buf[..8].copy_from_slice(GRAPH_MAGIC);
    buf[8..12].copy_from_slice(&header.version.to_le_bytes());
    buf[12..16].copy_from_slice(&header.live_nodes.to_le_bytes());
    buf[16..20].copy_from_slice(&header.live_edges.to_le_bytes());
    buf[20..24].copy_from_slice(&header.file_count.to_le_bytes());
    buf
}

fn parse_header(buf: &[u8]) -> Result<GraphHeader> {
    if buf.len() < HEADER_LEN {
        bail!("binary graph header truncated ({} < {HEADER_LEN} bytes)", buf.len());
    }
    if &buf[..8] != GRAPH_MAGIC {
        bail!("invalid binary graph magic header");
    }
    let version = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    if version != GRAPH_VERSION {
        bail!("unsupported binary graph version {version} (expected {GRAPH_VERSION})");
    }
    Ok(GraphHeader {
        version,
        live_nodes: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
        live_edges: u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]),
        file_count: u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]),
    })
}
