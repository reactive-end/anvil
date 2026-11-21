//! Core serializable graph structures and fast in-memory adjacency/resolution indexes.

mod serde_fxmap;

use anvil_core::traits::{EdgeKind, Language, SymbolKind};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

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

/// A directed edge connecting two nodes (or an external unresolved target).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub kind: GraphEdgeKind,
    pub line: u32,
    pub resolved: bool,
    pub raw_target: String,
}

/// Complete serializable code graph with O(1) in-memory forward and reverse adjacency indexes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodeGraph {
    pub repo_root: String,
    #[serde(with = "serde_fxmap")]
    pub file_hashes: FxHashMap<String, String>,
    #[serde(with = "serde_fxmap")]
    pub nodes: FxHashMap<String, GraphNode>,
    pub edges: Vec<GraphEdge>,
    #[serde(skip)]
    pub forward_adj: FxHashMap<String, Vec<usize>>,
    #[serde(skip)]
    pub reverse_adj: FxHashMap<String, Vec<usize>>,
    #[serde(skip)]
    pub file_nodes: FxHashMap<String, Vec<String>>,
    #[serde(skip)]
    pub symbol_index: FxHashMap<String, Vec<String>>,
    #[serde(skip)]
    pub angular_selectors: FxHashMap<String, String>,
    #[serde(skip)]
    pub kotlin_packages: FxHashMap<String, Vec<String>>,
    #[serde(skip)]
    pub file_packages: FxHashMap<String, String>,
    #[serde(skip)]
    pub tombstone_edges: usize,
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

    /// Compacts any tombstoned edges left behind by O(1) incremental updates.
    pub fn compact_tombstones(&mut self) {
        if self.tombstone_edges == 0 {
            return;
        }
        self.edges.retain(|e| !e.source.is_empty());
        self.tombstone_edges = 0;
        self.rebuild_indexes();
    }

    /// Rebuilds all in-memory adjacency, file-to-node, and symbol-lookup indexes in O(V + E).
    pub fn rebuild_indexes(&mut self) {
        self.forward_adj.clear();
        self.reverse_adj.clear();
        self.file_nodes.clear();
        self.symbol_index.clear();
        self.angular_selectors.clear();
        self.kotlin_packages.clear();
        self.file_packages.clear();

        self.forward_adj.reserve(self.nodes.len());
        self.reverse_adj.reserve(self.nodes.len());

        for (id, node) in &self.nodes {
            if !node.file.is_empty() {
                self.file_nodes
                    .entry(node.file.clone())
                    .or_default()
                    .push(id.clone());
            }
            match node.kind {
                NodeKind::File => {
                    if let Some(sig) = &node.signature
                        && let Some(pkg) = sig.strip_prefix("pkg:")
                    {
                        self.file_packages.insert(node.file.clone(), pkg.to_owned());
                        self.kotlin_packages
                            .entry(pkg.to_owned())
                            .or_default()
                            .push(node.file.clone());
                    }
                }
                NodeKind::Symbol => {
                    self.symbol_index
                        .entry(node.label.clone())
                        .or_default()
                        .push(id.clone());
                    let lower = node.label.to_ascii_lowercase();
                    if lower != node.label {
                        self.symbol_index.entry(lower).or_default().push(id.clone());
                    }
                    if node.signature.as_deref() == Some("angular_selector") {
                        self.angular_selectors
                            .insert(node.label.clone(), id.clone());
                    }
                }
                NodeKind::Semantic => {}
            }
        }

        for (idx, edge) in self.edges.iter().enumerate() {
            if edge.source.is_empty() {
                continue;
            }
            self.forward_adj
                .entry(edge.source.clone())
                .or_default()
                .push(idx);
            self.reverse_adj
                .entry(edge.target.clone())
                .or_default()
                .push(idx);
        }
    }

    /// Inserts a node, preserving deterministic IDs on symbol overload collisions.
    pub fn insert_symbol_node(&mut self, mut node: GraphNode) -> String {
        if self.nodes.contains_key(&node.id) {
            node.id = format!("{}@{}", node.id, node.line);
        }
        let id = node.id.clone();
        self.nodes.insert(id.clone(), node);
        id
    }

    /// Returns all outgoing edge structs for a given node ID.
    pub fn outgoing_edges(&self, node_id: &str) -> Vec<&GraphEdge> {
        self.forward_adj
            .get(node_id)
            .map(|idxs| {
                idxs.iter()
                    .filter_map(|&i| self.edges.get(i).filter(|e| !e.source.is_empty()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns all incoming edge structs for a given node ID.
    pub fn incoming_edges(&self, node_id: &str) -> Vec<&GraphEdge> {
        self.reverse_adj
            .get(node_id)
            .map(|idxs| {
                idxs.iter()
                    .filter_map(|&i| self.edges.get(i).filter(|e| !e.source.is_empty()))
                    .collect()
            })
            .unwrap_or_default()
    }
}
