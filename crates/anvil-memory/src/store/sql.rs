//! SQL row mapping and write helpers for `MemoryStore`.

use anvil_core::traits::FreshnessState;
use anyhow::Result;
use rusqlite::{OptionalExtension, Row, Transaction, params};

use crate::model::{MemoryChunkRecord, MemoryKind, Observation};

pub(crate) const OBS_COLUMNS: &str = "id, topic_key, kind, title, what, why, location, learned,
    related_files, related_symbols, freshness, freshness_reason,
    author, created_at, updated_at, expires_at";

pub(crate) fn find_topic_meta_in_tx(
    tx: &Transaction<'_>,
    topic_key: &str,
) -> Result<Option<(String, i64, i64)>> {
    let mut stmt = tx.prepare_cached(
        "SELECT id, created_at, updated_at FROM observations WHERE topic_key = ?1",
    )?;
    Ok(stmt
        .query_row([topic_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?)
}

pub(crate) fn insert_observation_row(
    tx: &Transaction<'_>,
    record: &MemoryChunkRecord,
    files_json: &str,
    symbols_json: &str,
) -> Result<()> {
    let mut stmt = tx.prepare_cached(
        "INSERT INTO observations (
             id, topic_key, kind, title, what, why, location, learned,
             related_files, related_symbols, freshness, freshness_reason,
             author, created_at, updated_at, expires_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'fresh', NULL, ?11, ?12, ?13, ?14)",
    )?;
    stmt.execute(params![
        record.id,
        record.topic_key,
        record.kind.as_str(),
        record.title,
        record.what,
        record.why,
        record.location,
        record.learned,
        files_json,
        symbols_json,
        record.author,
        record.created_at,
        record.updated_at,
        record.expires_at,
    ])?;
    Ok(())
}

pub(crate) fn update_observation_row(
    tx: &Transaction<'_>,
    existing_id: &str,
    record: &MemoryChunkRecord,
    files_json: &str,
    symbols_json: &str,
) -> Result<()> {
    let mut stmt = tx.prepare_cached(
        "UPDATE observations
         SET kind = ?1, title = ?2, what = ?3, why = ?4, location = ?5,
             learned = ?6, related_files = ?7, related_symbols = ?8,
             freshness = 'fresh', freshness_reason = NULL,
             author = ?9, updated_at = ?10, expires_at = ?11
         WHERE id = ?12",
    )?;
    stmt.execute(params![
        record.kind.as_str(),
        record.title,
        record.what,
        record.why,
        record.location,
        record.learned,
        files_json,
        symbols_json,
        record.author,
        record.updated_at,
        record.expires_at,
        existing_id,
    ])?;
    Ok(())
}

pub(crate) fn sync_observation_files(
    tx: &Transaction<'_>,
    obs_id: &str,
    files: &[String],
) -> Result<()> {
    let mut del_stmt =
        tx.prepare_cached("DELETE FROM observation_files WHERE observation_id = ?1")?;
    del_stmt.execute([obs_id])?;
    let mut ins_stmt = tx.prepare_cached(
        "INSERT OR IGNORE INTO observation_files (observation_id, file_path) VALUES (?1, ?2)",
    )?;
    for file in files {
        ins_stmt.execute(params![obs_id, file])?;
    }
    Ok(())
}

pub(crate) fn record_ingested_chunk_in_tx(
    tx: &Transaction<'_>,
    filename: &str,
    blake3_hash: &str,
    ingested_at: i64,
) -> Result<()> {
    let mut stmt = tx.prepare_cached(
        "INSERT INTO ingested_chunks (filename, blake3_hash, ingested_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(filename) DO UPDATE SET
             blake3_hash = excluded.blake3_hash,
             ingested_at = excluded.ingested_at",
    )?;
    stmt.execute(params![filename, blake3_hash, ingested_at])?;
    Ok(())
}

pub(crate) fn freshness_to_str(state: FreshnessState) -> &'static str {
    match state {
        FreshnessState::Fresh => "fresh",
        FreshnessState::NeedsVerification => "needs_verification",
        FreshnessState::Stale => "stale",
    }
}

pub(crate) fn parse_freshness(raw: &str) -> FreshnessState {
    match raw {
        "needs_verification" => FreshnessState::NeedsVerification,
        "stale" => FreshnessState::Stale,
        _ => FreshnessState::Fresh,
    }
}

pub(crate) fn row_to_observation(row: &Row<'_>) -> rusqlite::Result<Observation> {
    let kind_raw: String = row.get(2)?;
    let files_raw: String = row.get(8)?;
    let symbols_raw: String = row.get(9)?;
    let freshness_raw: String = row.get(10)?;
    Ok(Observation {
        id: row.get(0)?,
        topic_key: row.get(1)?,
        kind: kind_raw.parse().unwrap_or(MemoryKind::Decision),
        title: row.get(3)?,
        what: row.get(4)?,
        why: row.get(5)?,
        location: row.get(6)?,
        learned: row.get(7)?,
        related_files: serde_json::from_str(&files_raw).unwrap_or_default(),
        related_symbols: serde_json::from_str(&symbols_raw).unwrap_or_default(),
        freshness: parse_freshness(&freshness_raw),
        freshness_reason: row.get(11)?,
        author: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        expires_at: row.get(15)?,
    })
}
