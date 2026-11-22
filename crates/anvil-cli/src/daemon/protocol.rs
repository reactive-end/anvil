//! Newline-delimited JSON IPC protocol for the resident `anvil` code-graph daemon.

use anvil_graph::impact::BlastRadiusReport;
use anvil_graph::query::{QueryHit, RepoOutline, SymbolSlice};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Persisted connection metadata stored atomically in `.harness/.state/daemon.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonState {
    pub pid: u32,
    pub port: u16,
    pub token: String,
    pub started_at: DateTime<Utc>,
}

/// Authenticated single-line JSON request envelope sent to the local TCP daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonEnvelope {
    pub token: String,
    pub request: DaemonRequest,
}

/// Operations supported by the resident `CodeGraphEngine` daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DaemonRequest {
    Ping,
    Shutdown,
    UpdateFile { rel_path: String },
    Query { term: String, limit: usize },
    Outline { target: Option<String>, max_tokens: usize },
    ReadSymbol { symbol: String, include_imports: bool },
    BlastRadius { files: Vec<String>, depth: usize },
}

/// Single-line JSON response emitted by the resident daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DaemonResponse {
    Pong { pid: u32, nodes: usize, edges: usize },
    ShuttingDown,
    Updated { rel_path: String, changed: bool },
    QueryHits { hits: Vec<QueryHit> },
    OutlineResult { outline: RepoOutline },
    SymbolSliceResult { slice: Option<SymbolSlice> },
    BlastResult { report: BlastRadiusReport },
    Error { message: String },
}
