//! Core serializable graph structures with `u32` `NodeId` interning and compact edges.

mod indexes;
mod serde_fxmap;

use anvil_core::traits::{EdgeKind, Language, SymbolKind};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

/// Integer node index into `CodeGraph::nodes`.
pub type NodeId = u32;

/// Sentinel `NodeId` marking a tombstoned or unresolved slot.
pub const INVALID_NODE_ID: u32 = u32::MAX;

/// High-bit flag in `CompactEdge::target` indicating an index into `CodeGraph::externals`.
pub const EXTERNAL_TARGET_FLAG: u32 = 0x8000_0000;

/// Category of a node in the `anvil-graph` code graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    File,
    Symbol,
    Semantic,
}

/// Directed relationship kind between nodes in the code graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphEdgeKind {
    Contains,
    Imports,
    Calls,
    UsesProp,
    RendersTemplate,
    Provides,
    Injects,
    Extends,
    Implements,
    ReExports,
    SemanticLink,
}

impl From<EdgeKind> for GraphEdgeKind {
    fn from(value: EdgeKind) -> Self {
        match value {
            EdgeKind::Imports => Self::Imports,
            EdgeKind::Calls => Self::Calls,
            EdgeKind::UsesProp => Self::UsesProp,
            EdgeKind::RendersTemplate => Self::RendersTemplate,
            EdgeKind::Provides => Self::Provides,
            EdgeKind::Injects => Self::Injects,
            EdgeKind::Extends => Self::Extends,
            EdgeKind::Implements => Self::Implements,
            EdgeKind::ReExports => Self::ReExports,
        }
    }
}

/// A file, AST symbol, or overlaid semantic concept node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    pub file: String,
    pub line: u32,
    pub language: Option<Language>,
    pub symbol_kind: Option<SymbolKind>,
    pub signature: Option<String>,
    pub exported: bool,
    pub community: Option<usize>,
}

impl GraphNode {
    /// Returns `true` when this node slot has been tombstoned by an incremental removal.
    pub fn is_tombstone(&self) -> bool {
        self.id.is_empty()
    }
}

/// Compact 16-byte directed edge with zero heap allocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactEdge {
    pub source: u32,
    pub target: u32,
    pub kind: GraphEdgeKind,
    pub line: u32,
}

impl CompactEdge {
    /// Returns `true` if this edge has been tombstoned by an incremental removal.
    pub fn is_tombstone(self) -> bool {
        self.source == INVALID_NODE_ID
    }

    /// Returns `true` if `target` points to a live internal `GraphNode` index.
    pub fn is_resolved(self) -> bool {
        self.source != INVALID_NODE_ID
            && self.target != INVALID_NODE_ID
            && (self.target & EXTERNAL_TARGET_FLAG) == 0
    }

    /// Returns the index into `CodeGraph::externals` if this edge targets an external string.
    pub fn external_index(self) -> Option<usize> {
        if self.source == INVALID_NODE_ID
            || self.target == INVALID_NODE_ID
            || (self.target & EXTERNAL_TARGET_FLAG) == 0
        {
            return None;
        }
        Some((self.target & !EXTERNAL_TARGET_FLAG) as usize)
    }
}

/// Complete serializable code graph with `u32`-indexed adjacency and O(1) lookup indexes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodeGraph {
    pub repo_root: String,
    #[serde(with = "serde_fxmap")]
    pub file_hashes: FxHashMap<String, String>,
    #[serde(with = "serde_fxmap", default)]
    pub file_stats: FxHashMap<String, (u64, u64)>,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<CompactEdge>,
    #[serde(default)]
    pub externals: Vec<String>,
    #[serde(skip)]
    pub semantic_index: FxHashMap<String, u32>,
    #[serde(skip)]
    pub forward_adj: Vec<Vec<u32>>,
    #[serde(skip)]
    pub reverse_adj: Vec<Vec<u32>>,
    #[serde(skip)]
    pub file_nodes: FxHashMap<String, Vec<u32>>,
    #[serde(skip)]
    pub symbol_index: FxHashMap<String, Vec<u32>>,
    #[serde(skip)]
    pub angular_selectors: FxHashMap<String, u32>,
    #[serde(skip)]
    pub kotlin_packages: FxHashMap<String, Vec<String>>,
    #[serde(skip)]
    pub file_packages: FxHashMap<String, String>,
    #[serde(skip)]
    pub external_index: FxHashMap<String, u32>,
    #[serde(skip)]
    pub tombstone_edges: usize,
    #[serde(skip)]
    pub tombstone_nodes: usize,
}

impl CodeGraph {
    /// Creates an empty graph bound to a normalized repository root string.
    pub fn new(repo_root: impl Into<String>) -> Self {
        Self {
            repo_root: repo_root.into(),
            ..Self::default()
        }
    }

    /// Canonical node ID for a repository-relative file path.
    pub fn file_node_id(rel_path: &str) -> String {
        format!("file:{rel_path}")
    }

    /// Canonical node ID for a symbol within a repository-relative file.
    pub fn symbol_node_id(rel_path: &str, qualified_name: &str) -> String {
        format!("sym:{rel_path}::{qualified_name}")
    }

    /// Canonical node ID for an overlaid semantic/rationale node.
    pub fn semantic_node_id(id: &str) -> String {
        format!("sem:{id}")
    }

    /// Normalizes a path string to forward slashes without leading `./`.
    pub fn normalize_rel_path(path: &str) -> String {
        let slashed = path.replace('\\', "/");
        slashed.strip_prefix("./").unwrap_or(&slashed).to_owned()
    }

    /// Looks up a live `GraphNode` and its `NodeId` by canonical string ID without a global map.
    pub fn node_by_id(&self, id: &str) -> Option<(NodeId, &GraphNode)> {
        if let Some(rel) = id.strip_prefix("file:")
            && let Some(nids) = self.file_nodes.get(rel)
        {
            for &nid in nids {
                if let Some(node) = self.get_node(nid)
                    && node.id == id
                {
                    return Some((nid, node));
                }
            }
        }
        if let Some(rest) = id.strip_prefix("sym:")
            && let Some((rel, _)) = rest.split_once("::")
            && let Some(nids) = self.file_nodes.get(rel)
        {
            for &nid in nids {
                if let Some(node) = self.get_node(nid)
                    && node.id == id
                {
                    return Some((nid, node));
                }
            }
        }
        let &nid = self.semantic_index.get(id)?;
        let node = self.get_node(nid)?;
        Some((nid, node))
    }

    /// Looks up a live `GraphNode` by `NodeId`.
    pub fn get_node(&self, nid: NodeId) -> Option<&GraphNode> {
        self.nodes.get(nid as usize).filter(|n| !n.is_tombstone())
    }

    /// Returns `true` if a live node with canonical string ID `id` exists.
    pub fn contains_node_id(&self, id: &str) -> bool {
        self.node_by_id(id).is_some()
    }

    /// Interns an external target string, returning a flagged `CompactEdge::target`.
    pub fn intern_external(&mut self, raw: &str) -> u32 {
        if self.external_index.is_empty() && !self.externals.is_empty() {
            self.external_index.reserve(self.externals.len());
            for (idx, ext) in self.externals.iter().enumerate() {
                self.external_index.insert(ext.clone(), idx as u32);
            }
        }
        if let Some(&idx) = self.external_index.get(raw) {
            return idx | EXTERNAL_TARGET_FLAG;
        }
        let idx = u32::try_from(self.externals.len()).unwrap_or(0);
        self.externals.push(raw.to_owned());
        self.external_index.insert(raw.to_owned(), idx);
        idx | EXTERNAL_TARGET_FLAG
    }

    /// Resolves an external target flag value back to its string slice.
    pub fn external_str(&self, target: u32) -> Option<&str> {
        if target == INVALID_NODE_ID || (target & EXTERNAL_TARGET_FLAG) == 0 {
            return None;
        }
        let idx = (target & !EXTERNAL_TARGET_FLAG) as usize;
        self.externals.get(idx).map(String::as_str)
    }

    /// Returns a human-readable target name for a `CompactEdge`.
    pub fn edge_target_label(&self, edge: &CompactEdge) -> &str {
        if edge.is_resolved() {
            self.nodes
                .get(edge.target as usize)
                .map_or("", |n| n.label.as_str())
        } else {
            self.external_str(edge.target).unwrap_or("")
        }
    }

    /// Returns all live outgoing edges for `nid`.
    pub fn outgoing_edges(&self, nid: NodeId) -> Vec<&CompactEdge> {
        self.forward_adj
            .get(nid as usize)
            .map(|idxs| {
                idxs.iter()
                    .filter_map(|&i| self.edges.get(i as usize).filter(|e| !e.is_tombstone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns all live incoming edges for `nid`.
    pub fn incoming_edges(&self, nid: NodeId) -> Vec<&CompactEdge> {
        self.reverse_adj
            .get(nid as usize)
            .map(|idxs| {
                idxs.iter()
                    .filter_map(|&i| self.edges.get(i as usize).filter(|e| !e.is_tombstone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}
