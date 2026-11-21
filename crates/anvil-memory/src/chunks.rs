//! Conflict-free Git sync via immutable JSONL chunks in `.harness/memory/chunks/`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use anvil_core::fs_util::{atomic_write, sanitize_author};
use chrono::Utc;
use rustc_hash::FxHashSet;

use crate::model::{MemoryChunkRecord, SyncStats};
use crate::redactor;
use crate::store::MemoryStore;

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
    records: Vec<MemoryChunkRecord>,
}

/// Idempotently ingests all new `*.jsonl` chunks in a single SQLite transaction, skipping known files.
pub fn sync_chunks(chunks_dir: &Path, store: &mut MemoryStore) -> Result<SyncStats> {
    if !chunks_dir.exists() {
        return Ok(SyncStats::default());
    }

    let now_ms = Utc::now().timestamp_millis();
    let chunk_files = list_sorted_chunk_files(chunks_dir)?;
    let ingested_names = store.ingested_chunk_filenames()?;
    let pending = collect_pending_chunks(&chunk_files, &ingested_names, now_ms)?;

    let mut stats = SyncStats {
        files_scanned: chunk_files.len(),
        files_ingested: pending.len(),
        records_applied: 0,
    };
    if pending.is_empty() {
        return Ok(stats);
    }

    let mut all_records = Vec::new();
    let mut file_hashes = Vec::with_capacity(pending.len());
    for chunk in pending {
        file_hashes.push((chunk.filename, chunk.blake3_hash));
        all_records.extend(chunk.records);
    }
    all_records.sort_by(|a, b| a.updated_at.cmp(&b.updated_at).then_with(|| a.id.cmp(&b.id)));

    let tx = store.conn_mut().transaction()?;
    for record in &all_records {
        if MemoryStore::upsert_in_tx(&tx, record)?.is_some() {
            stats.records_applied += 1;
        }
    }
    for (filename, hash) in &file_hashes {
        MemoryStore::record_ingested_chunk_in_tx(&tx, filename, hash, now_ms)?;
    }
    tx.commit()?;
    Ok(stats)
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

fn collect_pending_chunks(
    files: &[PathBuf],
    ingested_names: &FxHashSet<String>,
    now_ms: i64,
) -> Result<Vec<PendingChunk>> {
    let mut pending = Vec::new();
    for path in files {
        let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if ingested_names.contains(filename) {
            continue;
        }
        let bytes =
            fs::read(path).with_context(|| format!("failed to read chunk {}", path.display()))?;
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let records = parse_jsonl_records(&bytes, path, now_ms)?;
        pending.push(PendingChunk {
            filename: filename.to_owned(),
            blake3_hash: hash,
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
