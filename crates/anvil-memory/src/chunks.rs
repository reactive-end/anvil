//! Conflict-free Git sync via immutable JSONL chunks in `.harness/memory/chunks/`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use anvil_core::fs_util::{atomic_write, sanitize_author};
use chrono::Utc;
use rusqlite::TransactionBehavior;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::model::{MemoryChunkRecord, SyncStats};
use crate::redactor;
use crate::store::sql::{delete_expired_observations_in_tx, delete_removed_chunk_in_tx};
use crate::store::{IngestedChunkMeta, MemoryStore};

/// Writes an immutable JSONL chunk file `<author>-<ulid>.jsonl` atomically and returns `(path, blake3_hex)`.
pub fn append_record(
    chunks_dir: &Path,
    author: &str,
    record: &MemoryChunkRecord,
) -> Result<(PathBuf, String)> {
    let safe_author = sanitize_author(author);
    let filename = format!("{safe_author}-{}.jsonl", ulid::Ulid::new());
    let file_path = chunks_dir.join(&filename);
    let mut line = serde_json::to_string(record).context("failed to serialize chunk record")?;
    line.push('\n');

    let hash = blake3::hash(line.as_bytes()).to_hex().to_string();
    atomic_write(&file_path, line.as_bytes())
        .with_context(|| format!("failed to write chunk file {}", file_path.display()))?;
    Ok((file_path, hash))
}

struct PendingChunk {
    filename: String,
    blake3_hash: String,
    mtime_ns: i64,
    byte_len: i64,
    records: Vec<(String, MemoryChunkRecord)>,
}

/// Idempotently ingests new or modified `*.jsonl` chunks in an IMMEDIATE SQLite transaction,
/// skips unchanged `(mtime_ns, byte_len)` / hashes, cleans deleted-chunk orphans and expired rows.
pub fn sync_chunks(chunks_dir: &Path, store: &mut MemoryStore) -> Result<SyncStats> {
    let now_ms = Utc::now().timestamp_millis();
    let chunk_files = if chunks_dir.exists() {
        list_sorted_chunk_files(chunks_dir)?
    } else {
        Vec::new()
    };
    let ingested_meta = store.ingested_chunk_meta()?;
    let pending = collect_pending_chunks(&chunk_files, &ingested_meta, now_ms)?;
    let removed_chunks = collect_removed_chunks(&chunk_files, &ingested_meta);
    let has_expired = store.has_expired_observations(now_ms)?;

    let mut stats = SyncStats {
        files_scanned: chunk_files.len(),
        files_ingested: pending.len(),
        records_applied: 0,
    };
    if pending.is_empty() && removed_chunks.is_empty() && !has_expired {
        return Ok(stats);
    }

    apply_sync_tx(store, pending, &removed_chunks, now_ms, &mut stats)?;
    Ok(stats)
}

fn apply_sync_tx(
    store: &mut MemoryStore,
    pending: Vec<PendingChunk>,
    removed_chunks: &[String],
    now_ms: i64,
    stats: &mut SyncStats,
) -> Result<()> {
    let mut all_records = Vec::new();
    let mut file_meta = Vec::with_capacity(pending.len());
    for chunk in pending {
        file_meta.push((chunk.filename, chunk.blake3_hash, chunk.mtime_ns, chunk.byte_len));
        all_records.extend(chunk.records);
    }
    all_records.sort_by(|a, b| a.1.updated_at.cmp(&b.1.updated_at).then_with(|| a.1.id.cmp(&b.1.id)));

    let tx = store.conn_mut().transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (filename, record) in &all_records {
        if MemoryStore::upsert_in_tx_with_chunk(&tx, record, Some(filename.as_str()))?.is_some() {
            stats.records_applied += 1;
        }
    }
    for (filename, hash, mtime_ns, byte_len) in &file_meta {
        MemoryStore::record_ingested_chunk_meta_in_tx(&tx, filename, hash, now_ms, *mtime_ns, *byte_len)?;
    }
    for old_file in removed_chunks {
        delete_removed_chunk_in_tx(&tx, old_file)?;
    }
    delete_expired_observations_in_tx(&tx, now_ms)?;
    tx.commit()?;
    Ok(())
}

fn collect_removed_chunks(
    files: &[PathBuf],
    ingested_meta: &FxHashMap<String, IngestedChunkMeta>,
) -> Vec<String> {
    let on_disk: FxHashSet<&str> = files
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .collect();
    ingested_meta
        .keys()
        .filter(|name| !on_disk.contains(name.as_str()))
        .cloned()
        .collect()
}

fn list_sorted_chunk_files(chunks_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(chunks_dir)
        .with_context(|| format!("failed to read chunks dir {}", chunks_dir.display()))?
    {
        let path = entry?.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

pub(crate) fn file_mtime_and_len(path: &Path) -> (i64, i64) {
    let Ok(meta) = fs::metadata(path) else {
        return (0, 0);
    };
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
    let byte_len = i64::try_from(meta.len()).unwrap_or(i64::MAX);
    (mtime_ns, byte_len)
}

fn collect_pending_chunks(
    files: &[PathBuf],
    ingested_meta: &FxHashMap<String, IngestedChunkMeta>,
    now_ms: i64,
) -> Result<Vec<PendingChunk>> {
    let mut pending = Vec::new();
    for path in files {
        let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let (mtime_ns, byte_len) = file_mtime_and_len(path);
        let cached = ingested_meta.get(filename);
        if let Some(prev) = cached
            && prev.mtime_ns > 0
            && prev.mtime_ns == mtime_ns
            && prev.byte_len == byte_len
        {
            continue;
        }
        let bytes =
            fs::read(path).with_context(|| format!("failed to read chunk {}", path.display()))?;
        let hash = blake3::hash(&bytes).to_hex().to_string();
        if cached.is_some_and(|prev| prev.blake3_hash == hash) {
            continue;
        }
        let records = parse_jsonl_records(&bytes, path, now_ms)?
            .into_iter()
            .map(|rec| (filename.to_owned(), rec))
            .collect();
        pending.push(PendingChunk {
            filename: filename.to_owned(),
            blake3_hash: hash,
            mtime_ns,
            byte_len,
            records,
        });
    }
    Ok(pending)
}

fn parse_jsonl_records(bytes: &[u8], path: &Path, now_ms: i64) -> Result<Vec<MemoryChunkRecord>> {
    let text = std::str::from_utf8(bytes)
        .with_context(|| format!("chunk file is not valid UTF-8: {}", path.display()))?;
    let mut records = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: MemoryChunkRecord = serde_json::from_str(trimmed).with_context(|| {
            format!(
                "invalid JSONL record at {}:{}",
                path.display(),
                idx + 1
            )
        })?;
        if parsed.expires_at.is_some_and(|exp| exp <= now_ms) {
            continue;
        }
        records.push(redactor::redact_record(&parsed));
    }
    Ok(records)
}
