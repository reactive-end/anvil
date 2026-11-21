//! SQLite persistence layer: topic_key upserts, TTL cleanup, and staleness updates.

pub(crate) mod sql;

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use anvil_core::traits::FreshnessState;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use rustc_hash::FxHashSet;

use crate::model::{MemoryChunkRecord, Observation, normalize_rel_path};
use crate::schema;
use sql::{
    find_topic_meta_in_tx, freshness_to_str, insert_observation_row, row_to_observation,
    sync_observation_files, update_observation_row,
};

/// SQLite-backed store for memory observations and ingested chunk hashes.
pub struct MemoryStore {
    conn: Connection,
}

impl MemoryStore {
    /// Opens or creates the SQLite database at `db_path` and initializes the schema.
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create SQLite parent dir {}", parent.display())
            })?;
        }
        let mut conn = Connection::open(db_path)
            .with_context(|| format!("failed to open SQLite DB at {}", db_path.display()))?;
        schema::init_schema(&mut conn)?;
        Ok(Self { conn })
    }

    /// Opens an isolated in-memory SQLite store (primarily for fast unit tests).
    pub fn open_in_memory() -> Result<Self> {
        let mut conn =
            Connection::open_in_memory().context("failed to open in-memory SQLite DB")?;
        schema::init_schema(&mut conn)?;
        Ok(Self { conn })
    }

    /// Returns a shared reference to the underlying `rusqlite::Connection`.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Returns a mutable reference to the underlying `rusqlite::Connection`.
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Upserts an observation by `topic_key` inside a dedicated transaction.
    pub fn upsert_observation(&mut self, record: &MemoryChunkRecord) -> Result<Observation> {
        let tx = self.conn.transaction()?;
        let applied = Self::upsert_in_tx(&tx, record)?;
        tx.commit()?;
        match applied {
            Some(obs) => Ok(obs),
            None => self
                .get_by_topic_key(&record.topic_key)?
                .context("observation missing after upsert"),
        }
    }

    /// Upserts a chunk record inside an existing transaction using last-writer-wins on `updated_at`.
    pub fn upsert_in_tx(
        tx: &Transaction<'_>,
        record: &MemoryChunkRecord,
    ) -> Result<Option<Observation>> {
        let existing = find_topic_meta_in_tx(tx, &record.topic_key)?;
        let normalized_files: Vec<String> = record
            .related_files
            .iter()
            .map(|f| normalize_rel_path(f))
            .filter(|f| !f.is_empty())
            .collect();
        let files_json = serde_json::to_string(&normalized_files)?;
        let symbols_json = serde_json::to_string(&record.related_symbols)?;

        let (obs_id, created_at) = match existing {
            Some((existing_id, existing_created, existing_updated)) => {
                if existing_updated > record.updated_at {
                    return Ok(None);
                }
                update_observation_row(tx, &existing_id, record, &files_json, &symbols_json)?;
                (existing_id, existing_created.min(record.created_at))
            }
            None => {
                insert_observation_row(tx, record, &files_json, &symbols_json)?;
                (record.id.clone(), record.created_at)
            }
        };

        sync_observation_files(tx, &obs_id, &normalized_files)?;
        Ok(Some(Observation {
            id: obs_id,
            topic_key: record.topic_key.clone(),
            kind: record.kind,
            title: record.title.clone(),
            what: record.what.clone(),
            why: record.why.clone(),
            location: record.location.clone(),
            learned: record.learned.clone(),
            related_files: normalized_files,
            related_symbols: record.related_symbols.clone(),
            freshness: FreshnessState::Fresh,
            freshness_reason: None,
            author: record.author.clone(),
            created_at,
            updated_at: record.updated_at,
            expires_at: record.expires_at,
        }))
    }

    /// Fetches a full non-expired observation by its ULID `id`.
    pub fn get_by_id(&self, id: &str) -> Result<Option<Observation>> {
        self.get_by_column("id", id, Utc::now().timestamp_millis())
    }

    /// Fetches a full non-expired observation by its unique `topic_key`.
    pub fn get_by_topic_key(&self, topic_key: &str) -> Result<Option<Observation>> {
        self.get_by_column("topic_key", topic_key, Utc::now().timestamp_millis())
    }

    fn get_by_column(&self, column: &str, value: &str, now_ms: i64) -> Result<Option<Observation>> {
        let query = format!(
            "SELECT {} FROM observations
             WHERE {column} = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
            sql::OBS_COLUMNS
        );
        let mut stmt = self.conn.prepare_cached(&query)?;
        Ok(stmt
            .query_row(params![value, now_ms], row_to_observation)
            .optional()?)
    }

    /// Lists all observations linked to at least one file or symbol.
    pub fn list_with_relations(&self) -> Result<Vec<Observation>> {
        let query = format!(
            "SELECT {} FROM observations WHERE related_files != '[]' OR related_symbols != '[]'",
            sql::OBS_COLUMNS
        );
        let mut stmt = self.conn.prepare_cached(&query)?;
        let rows = stmt.query_map([], row_to_observation)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Updates the freshness state and optional reason for an observation.
    pub fn update_freshness(
        &mut self,
        id: &str,
        state: FreshnessState,
        reason: Option<&str>,
    ) -> Result<bool> {
        let mut stmt = self.conn.prepare_cached(
            "UPDATE observations SET freshness = ?1, freshness_reason = ?2 WHERE id = ?3",
        )?;
        let changed = stmt.execute(params![freshness_to_str(state), reason, id])?;
        Ok(changed > 0)
    }

    /// Marks any `Fresh` observations referencing `changed_files` as `NeedsVerification`.
    pub fn mark_files_changed(&mut self, changed_files: &[String]) -> Result<usize> {
        if changed_files.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        let mut total_updated = 0usize;
        let mut seen = FxHashSet::default();
        {
            let mut stmt = tx.prepare_cached(
                "UPDATE observations SET freshness = 'needs_verification', freshness_reason = ?1
                 WHERE id IN (SELECT observation_id FROM observation_files WHERE file_path = ?2)
                   AND freshness = 'fresh'",
            )?;
            for raw_path in changed_files {
                let norm = normalize_rel_path(raw_path);
                if norm.is_empty() || !seen.insert(norm.clone()) {
                    continue;
                }
                let reason = format!("related file modified: {norm}");
                total_updated += stmt.execute(params![reason, norm])?;
            }
        }
        tx.commit()?;
        Ok(total_updated)
    }

    /// Removes expired `Session` observations while preserving all permanent categories.
    pub fn delete_expired_sessions(&mut self, now_ms: i64) -> Result<usize> {
        let mut stmt = self.conn.prepare_cached(
            "DELETE FROM observations
             WHERE kind = 'session' AND expires_at IS NOT NULL AND expires_at <= ?1",
        )?;
        Ok(stmt.execute([now_ms])?)
    }

    /// Returns the set of already-ingested chunk filenames for O(new_files) sync skipping.
    pub fn ingested_chunk_filenames(&self) -> Result<FxHashSet<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT filename FROM ingested_chunks")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut names = FxHashSet::default();
        for row in rows {
            names.insert(row?);
        }
        Ok(names)
    }

    /// Records or updates an ingested chunk filename and BLAKE3 hash inside a transaction.
    pub fn record_ingested_chunk_in_tx(
        tx: &Transaction<'_>,
        filename: &str,
        blake3_hash: &str,
        ingested_at: i64,
    ) -> Result<()> {
        sql::record_ingested_chunk_in_tx(tx, filename, blake3_hash, ingested_at)
    }
}
