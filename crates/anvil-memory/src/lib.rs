//! Local-first persistent memory engine with SQLite FTS5, redaction, and Git chunks.

pub mod chunks;
pub mod model;
pub mod query;
pub mod redactor;
pub mod schema;
pub mod staleness;
pub mod store;

use std::path::{Path, PathBuf};

use anyhow::Result;
use anvil_core::traits::SymbolFreshnessProvider;
use chrono::Utc;
use rusqlite::TransactionBehavior;

pub use model::{
    MemoryChunkRecord, MemoryKind, Observation, ObservationInput, SearchHit, StalenessUpdate,
    SyncStats, TimelineEntry,
};
use model::{derive_topic_key, normalize_rel_path};
use store::MemoryStore;

const DEFAULT_SESSION_TTL_SECONDS: i64 = 7 * 24 * 60 * 60;

/// High-level facade combining SQLite FTS5 store, Git chunks, redactor, and staleness checks.
pub struct MemoryEngine {
    store: MemoryStore,
    chunks_dir: PathBuf,
    author: String,
    last_sync: SyncStats,
}

impl MemoryEngine {
    /// Opens the repository memory engine at `.harness/.state/anvil.db` and `.harness/memory/chunks/`.
    pub fn open(project_root: &Path, author: impl Into<String>) -> Result<Self> {
        let db_path = project_root.join(".harness/.state/anvil.db");
        let chunks_dir = project_root.join(".harness/memory/chunks");
        Self::with_paths(&db_path, &chunks_dir, author)
    }

    /// Opens the repository memory engine without running initial chunk sync.
    pub fn open_without_sync(project_root: &Path, author: impl Into<String>) -> Result<Self> {
        let db_path = project_root.join(".harness/.state/anvil.db");
        let chunks_dir = project_root.join(".harness/memory/chunks");
        Self::with_paths_without_sync(&db_path, &chunks_dir, author)
    }

    /// Opens the memory engine using explicit SQLite DB and chunk directory paths, syncing chunks.
    pub fn with_paths(
        db_path: &Path,
        chunks_dir: &Path,
        author: impl Into<String>,
    ) -> Result<Self> {
        let mut engine = Self::with_paths_without_sync(db_path, chunks_dir, author)?;
        engine.sync()?;
        Ok(engine)
    }

    /// Opens the memory engine using explicit paths without syncing chunks.
    pub fn with_paths_without_sync(
        db_path: &Path,
        chunks_dir: &Path,
        author: impl Into<String>,
    ) -> Result<Self> {
        let store = MemoryStore::open(db_path)?;
        Ok(Self {
            store,
            chunks_dir: chunks_dir.to_path_buf(),
            author: author.into(),
            last_sync: SyncStats::default(),
        })
    }

    /// Returns the statistics from the most recent chunk sync pass.
    pub fn last_sync_stats(&self) -> SyncStats {
        self.last_sync
    }

    /// Redacts, appends a Git chunk for permanent kinds, and upserts the observation in SQLite.
    pub fn save(&mut self, input: ObservationInput) -> Result<Observation> {
        let record = self.build_record(input)?;
        let chunk_meta = if record.kind == MemoryKind::Session {
            None
        } else {
            Some(chunks::append_record(&self.chunks_dir, &self.author, &record)?)
        };

        let chunk_filename = chunk_meta
            .as_ref()
            .and_then(|(p, _)| p.file_name().and_then(|n| n.to_str()).map(str::to_owned));
        let tx = self.store.conn_mut().transaction_with_behavior(TransactionBehavior::Immediate)?;
        let obs = MemoryStore::upsert_in_tx_with_chunk(&tx, &record, chunk_filename.as_deref())?;
        if let Some((chunk_path, hash)) = &chunk_meta
            && let Some(filename) = chunk_path.file_name().and_then(|n| n.to_str())
        {
            let (mtime_ns, byte_len) = chunks::file_mtime_and_len(chunk_path);
            MemoryStore::record_ingested_chunk_meta_in_tx(
                &tx,
                filename,
                hash,
                record.updated_at,
                mtime_ns,
                byte_len,
            )?;
        }
        tx.commit()?;

        match obs {
            Some(saved) => Ok(saved),
            None => self
                .store
                .get_by_topic_key(&record.topic_key)?
                .ok_or_else(|| anyhow::anyhow!("saved observation missing from store")),
        }
    }

    fn build_record(&self, input: ObservationInput) -> Result<MemoryChunkRecord> {
        let clean = redactor::redact_input(&input);
        let topic_key = clean
            .topic_key
            .filter(|k| !k.trim().is_empty())
            .unwrap_or_else(|| derive_topic_key(clean.kind, &clean.title));

        let now_ms = Utc::now().timestamp_millis();
        let existing = self.store.get_by_topic_key(&topic_key)?;
        let updated_at = match &existing {
            Some(prev) if prev.updated_at >= now_ms => prev.updated_at + 1,
            _ => now_ms,
        };
        let created_at = existing.as_ref().map_or(updated_at, |p| p.created_at);
        let id = existing
            .as_ref()
            .map_or_else(|| ulid::Ulid::new().to_string(), |p| p.id.clone());

        let ttl_secs = match (clean.kind, clean.ttl_seconds) {
            (_, Some(secs)) => Some(secs),
            (MemoryKind::Session, None) => Some(DEFAULT_SESSION_TTL_SECONDS),
            _ => None,
        };
        let expires_at = ttl_secs.map(|s| updated_at + s.saturating_mul(1000));

        Ok(MemoryChunkRecord {
            id,
            topic_key,
            kind: clean.kind,
            title: clean.title,
            what: clean.what,
            why: clean.why,
            location: clean.location,
            learned: clean.learned,
            related_files: clean
                .related_files
                .iter()
                .map(|f| normalize_rel_path(f))
                .filter(|f| !f.is_empty())
                .collect(),
            related_symbols: clean.related_symbols,
            author: self.author.clone(),
            created_at,
            updated_at,
            expires_at,
        })
    }

    /// Searches memory via FTS5 BM25 returning compact hits for progressive disclosure.
    pub fn search(
        &self,
        query_text: &str,
        kind: Option<MemoryKind>,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        query::search(self.store.conn(), query_text, kind, limit)
    }

    /// Returns chronological neighborhood entries around an observation `id` or `topic_key`.
    pub fn timeline(
        &self,
        id_or_topic: &str,
        before: usize,
        after: usize,
    ) -> Result<Vec<TimelineEntry>> {
        query::timeline(self.store.conn(), id_or_topic, before, after)
    }

    /// Loads the full observation body by `id`.
    pub fn get(&self, id: &str) -> Result<Option<Observation>> {
        query::get(self.store.conn(), id)
    }

    /// Loads the full observation body by `topic_key`.
    pub fn get_by_topic(&self, topic_key: &str) -> Result<Option<Observation>> {
        self.store.get_by_topic_key(topic_key)
    }

    /// Returns compact session startup context within `max_tokens`.
    pub fn session_context(&self, max_tokens: usize) -> Result<Vec<SearchHit>> {
        let now_ms = Utc::now().timestamp_millis();
        query::session_context(self.store.conn(), max_tokens, now_ms)
    }

    /// Re-syncs any newly arrived Git chunks into SQLite idempotently.
    pub fn sync(&mut self) -> Result<SyncStats> {
        let stats = chunks::sync_chunks(&self.chunks_dir, &mut self.store)?;
        self.last_sync = stats;
        Ok(stats)
    }

    /// Evaluates linked observations against a `SymbolFreshnessProvider` (e.g. `anvil-graph`).
    pub fn check_staleness(
        &mut self,
        provider: &dyn SymbolFreshnessProvider,
    ) -> Result<Vec<StalenessUpdate>> {
        staleness::evaluate_staleness(&mut self.store, provider)
    }

    /// Fast PostToolUse invalidation marking observations tied to `changed_files` as `NeedsVerification`.
    pub fn mark_files_changed(&mut self, changed_files: &[String]) -> Result<usize> {
        self.store.mark_files_changed(changed_files)
    }

    /// Cleans up expired `Session` observations using the current wall-clock timestamp.
    pub fn cleanup_expired_sessions(&mut self) -> Result<usize> {
        self.cleanup_expired_sessions_at(Utc::now().timestamp_millis())
    }

    /// Cleans up expired `Session` observations as of `now_ms`.
    pub fn cleanup_expired_sessions_at(&mut self, now_ms: i64) -> Result<usize> {
        self.store.delete_expired_sessions(now_ms)
    }
}
