//! Domain types for persistent team memory, progressive disclosure, and Git chunks.

use std::fmt;
use std::str::FromStr;

use anyhow::{Result, bail};
use anvil_core::traits::FreshnessState;
use serde::{Deserialize, Serialize};

/// Allowed semantic categories for persistent memory observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Decision,
    Gotcha,
    Domain,
    Session,
}

impl MemoryKind {
    /// Returns the canonical lowercase storage representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Gotcha => "gotcha",
            Self::Domain => "domain",
            Self::Session => "session",
        }
    }
}

impl fmt::Display for MemoryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MemoryKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "decision" => Ok(Self::Decision),
            "gotcha" => Ok(Self::Gotcha),
            "domain" => Ok(Self::Domain),
            "session" => Ok(Self::Session),
            other => bail!("unknown memory kind: {other}"),
        }
    }
}

/// Caller-supplied payload for saving or upserting a memory observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationInput {
    #[serde(default)]
    pub topic_key: Option<String>,
    pub kind: MemoryKind,
    pub title: String,
    pub what: String,
    pub why: String,
    #[serde(rename = "where", alias = "location")]
    pub location: String,
    pub learned: String,
    #[serde(default)]
    pub related_files: Vec<String>,
    #[serde(default)]
    pub related_symbols: Vec<String>,
    #[serde(default)]
    pub ttl_seconds: Option<i64>,
}

impl ObservationInput {
    /// Constructs a new observation input with empty relations and no TTL override.
    pub fn new(
        kind: MemoryKind,
        title: impl Into<String>,
        what: impl Into<String>,
        why: impl Into<String>,
        location: impl Into<String>,
        learned: impl Into<String>,
    ) -> Self {
        Self {
            topic_key: None,
            kind,
            title: title.into(),
            what: what.into(),
            why: why.into(),
            location: location.into(),
            learned: learned.into(),
            related_files: Vec::new(),
            related_symbols: Vec::new(),
            ttl_seconds: None,
        }
    }
}

/// Full persisted memory observation returned on demand via `get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub topic_key: String,
    pub kind: MemoryKind,
    pub title: String,
    pub what: String,
    pub why: String,
    #[serde(rename = "where", alias = "location")]
    pub location: String,
    pub learned: String,
    pub related_files: Vec<String>,
    pub related_symbols: Vec<String>,
    pub freshness: FreshnessState,
    pub freshness_reason: Option<String>,
    pub author: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub expires_at: Option<i64>,
}

/// Compact search result for token-efficient progressive disclosure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub id: String,
    pub topic_key: String,
    pub kind: MemoryKind,
    pub title: String,
    pub updated_at: i64,
    pub freshness: FreshnessState,
    pub snippet: String,
}

/// Chronological neighborhood entry around a target observation or topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineEntry {
    pub id: String,
    pub topic_key: String,
    pub kind: MemoryKind,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub freshness: FreshnessState,
    pub is_anchor: bool,
}

/// Immutable JSONL record written to `.harness/memory/chunks/<author>-<ulid>.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryChunkRecord {
    pub id: String,
    pub topic_key: String,
    pub kind: MemoryKind,
    pub title: String,
    pub what: String,
    pub why: String,
    #[serde(rename = "where", alias = "location")]
    pub location: String,
    pub learned: String,
    #[serde(default)]
    pub related_files: Vec<String>,
    #[serde(default)]
    pub related_symbols: Vec<String>,
    pub author: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

/// Summary of a Git chunk synchronization pass into SQLite.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStats {
    pub files_scanned: usize,
    pub files_ingested: usize,
    pub records_applied: usize,
}

/// Records a freshness transition produced by `SymbolFreshnessProvider`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StalenessUpdate {
    pub id: String,
    pub topic_key: String,
    pub previous: FreshnessState,
    pub current: FreshnessState,
    pub reason: Option<String>,
}

/// Normalizes a relative file path to forward slashes without leading `./`.
pub fn normalize_rel_path(path: &str) -> String {
    let replaced = path.trim().replace('\\', "/");
    replaced.strip_prefix("./").unwrap_or(&replaced).to_owned()
}

/// Derives a stable topic key when the caller omits an explicit `topic_key`.
pub fn derive_topic_key(kind: MemoryKind, title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut prev_dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() {
            slug.push('-');
            prev_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        format!("{}-{}", kind.as_str(), ulid::Ulid::new())
    } else {
        format!("{}:{trimmed}", kind.as_str())
    }
}
