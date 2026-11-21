//! SQLite pragmas, relational tables, FTS5 virtual table, and sync triggers.

use anyhow::{Context, Result};
use rusqlite::Connection;

const PRAGMAS_SQL: &str = "
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
    expires_at INTEGER
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

CREATE TRIGGER IF NOT EXISTS observations_au AFTER UPDATE ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, title, topic_key, what, why, location, learned)
    VALUES ('delete', old.rowid, old.title, old.topic_key, old.what, old.why, old.location, old.learned);
    INSERT INTO observations_fts(rowid, title, topic_key, what, why, location, learned)
    VALUES (new.rowid, new.title, new.topic_key, new.what, new.why, new.location, new.learned);
END;

CREATE TABLE IF NOT EXISTS ingested_chunks (
    filename TEXT PRIMARY KEY,
    blake3_hash TEXT NOT NULL,
    ingested_at INTEGER NOT NULL
);
";

/// Applies performance-tuned SQLite pragmas (WAL, NORMAL sync, memory temp store, mmap).
pub fn apply_pragmas(conn: &Connection) -> Result<()> {
    conn.execute_batch(PRAGMAS_SQL)
        .context("failed to apply SQLite performance pragmas")
}

/// Initializes tables, indexes, FTS5 virtual table, and sync triggers idempotently.
pub fn init_schema(conn: &Connection) -> Result<()> {
    apply_pragmas(conn)?;
    conn.execute_batch(SCHEMA_SQL)
        .context("failed to initialize anvil-memory SQLite schema")
}
