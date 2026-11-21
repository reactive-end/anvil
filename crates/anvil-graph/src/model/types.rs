//! Core node, edge, and ID primitives for `CodeGraph`.

use anvil_core::traits::{EdgeKind, Language, SymbolKind};
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
