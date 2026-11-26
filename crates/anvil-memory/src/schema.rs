//! SQLite pragmas, relational tables, FTS5 virtual table, and sync triggers.

use anyhow::{Context, Result};
use rusqlite::{Connection, TransactionBehavior};

const CURRENT_SCHEMA_VERSION: i64 = 3;

const PRAGMAS_SQL: &str = "
PRAGMA busy_timeout = 5000;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA temp_store = MEMORY;
PRAGMA mmap_size = 67108864;
PRAGMA foreign_keys = ON;
";

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS observations (
    id TEXT PRIMARY KEY,
    topic_key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    what TEXT NOT NULL,
    why TEXT NOT NULL,
    location TEXT NOT NULL,
    learned TEXT NOT NULL,
    related_files TEXT NOT NULL,
    related_symbols TEXT NOT NULL,
    freshness TEXT NOT NULL,
    freshness_reason TEXT,
    author TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    expires_at INTEGER,
    chunk_filename TEXT
);

CREATE INDEX IF NOT EXISTS idx_observations_kind_updated
    ON observations(kind, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_observations_updated
    ON observations(updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_observations_expires
    ON observations(expires_at)
    WHERE expires_at IS NOT NULL;

CREATE TABLE IF NOT EXISTS observation_files (
    observation_id TEXT NOT NULL,
    file_path TEXT NOT NULL,
    PRIMARY KEY (observation_id, file_path),
    FOREIGN KEY (observation_id) REFERENCES observations(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_observation_files_path
    ON observation_files(file_path);

CREATE VIRTUAL TABLE IF NOT EXISTS observations_fts USING fts5(
    title,
    topic_key,
    what,
    why,
    location,
    learned,
    content='observations',
    content_rowid='rowid',
    tokenize='porter unicode61'
);

CREATE TRIGGER IF NOT EXISTS observations_ai AFTER INSERT ON observations BEGIN
    INSERT INTO observations_fts(rowid, title, topic_key, what, why, location, learned)
    VALUES (new.rowid, new.title, new.topic_key, new.what, new.why, new.location, new.learned);
END;

CREATE TRIGGER IF NOT EXISTS observations_ad AFTER DELETE ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, title, topic_key, what, why, location, learned)
    VALUES ('delete', old.rowid, old.title, old.topic_key, old.what, old.why, old.location, old.learned);
END;

DROP TRIGGER IF EXISTS observations_au;
CREATE TRIGGER observations_au AFTER UPDATE OF topic_key, title, what, why, location, learned ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, title, topic_key, what, why, location, learned)
    VALUES ('delete', old.rowid, old.title, old.topic_key, old.what, old.why, old.location, old.learned);
    INSERT INTO observations_fts(rowid, title, topic_key, what, why, location, learned)
    VALUES (new.rowid, new.title, new.topic_key, new.what, new.why, new.location, new.learned);
END;

CREATE TABLE IF NOT EXISTS ingested_chunks (
    filename TEXT PRIMARY KEY,
    blake3_hash TEXT NOT NULL,
    ingested_at INTEGER NOT NULL,
    mtime_ns INTEGER NOT NULL DEFAULT 0,
    byte_len INTEGER NOT NULL DEFAULT 0
);
";

/// Applies performance-tuned SQLite pragmas (WAL, NORMAL sync, memory temp store, mmap).
pub fn apply_pragmas(conn: &Connection) -> Result<()> {
    conn.execute_batch(PRAGMAS_SQL).context("failed to apply SQLite performance pragmas")
}

/// Initializes tables, indexes, FTS5 virtual table, and sync triggers idempotently.
pub fn init_schema(conn: &mut Connection) -> Result<()> {
    apply_pragmas(conn)?;
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("failed to read SQLite user_version")?;
    if version >= CURRENT_SCHEMA_VERSION {
        return Ok(());
    }

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to start IMMEDIATE schema migration transaction")?;
    let current_version: i64 = tx
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("failed to re-read SQLite user_version")?;
    if current_version < CURRENT_SCHEMA_VERSION {
        tx.execute_batch(SCHEMA_SQL).context("failed to initialize anvil-memory SQLite schema")?;
        migrate_v3_columns(&tx)?;
        tx.execute_batch(&format!("PRAGMA user_version = {CURRENT_SCHEMA_VERSION};"))
            .context("failed to update SQLite user_version")?;
    }
    tx.commit().context("failed to commit anvil-memory schema transaction")
}

fn migrate_v3_columns(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    add_column_if_missing(tx, "observations", "chunk_filename", "TEXT")?;
    add_column_if_missing(tx, "ingested_chunks", "mtime_ns", "INTEGER NOT NULL DEFAULT 0")?;
    add_column_if_missing(tx, "ingested_chunks", "byte_len", "INTEGER NOT NULL DEFAULT 0")?;
    tx.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_observations_chunk_filename
             ON observations(chunk_filename)
             WHERE chunk_filename IS NOT NULL;",
    )
    .context("failed to create idx_observations_chunk_filename")
}

fn add_column_if_missing(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
    col_def: &str,
) -> Result<()> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut stmt = tx.prepare(&pragma)?;
    let exists =
        stmt.query_map([], |row| row.get::<_, String>(1))?.flatten().any(|name| name == column);
    drop(stmt);
    if !exists {
        let alter = format!("ALTER TABLE {table} ADD COLUMN {column} {col_def}");
        tx.execute_batch(&alter)
            .with_context(|| format!("failed to add column {table}.{column}"))?;
    }
    Ok(())
}
