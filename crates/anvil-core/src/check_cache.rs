//! Content-addressed Tier 1 check cache persisted at `.harness/.state/check-cache.json`.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::fs_util::{atomic_write, with_file_lock};

const MAX_CACHE_ENTRIES: usize = 64;
const LOCK_TIMEOUT_MS: u64 = 2_000;
const ROOT_MANIFESTS: [&str; 4] = [
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "tsconfig.json",
];

/// Cached result of a Tier 1 verification pass keyed by content-addressed scope hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedTier1 {
    pub key: String,
    pub passed: bool,
    pub summary: String,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CheckCacheStore {
    #[serde(default)]
    entries: Vec<CachedTier1>,
}

/// Computes a deterministic `blake3` hex digest over sorted `(rel_path, blake3_bytes)` for
/// `scope_files`, root manifests (`tsconfig.json`, `Cargo.toml`, `package.json`, `pyproject.toml`),
/// and `extra_salt`.
pub fn compute_scope_key(
    repo_root: &Path,
    scope_files: &[String],
    extra_salt: &str,
) -> Result<String> {
    let mut normalized = BTreeSet::new();
    for raw in scope_files {
        let norm = normalize_rel_path(raw);
        if !norm.is_empty() {
            normalized.insert(norm);
        }
    }

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"anvil-tier1-cache-v1\0salt:");
    hasher.update(extra_salt.as_bytes());
    hasher.update(b"\0");

    for rel in &normalized {
        hash_named_file(&mut hasher, b"file:", repo_root, rel)?;
    }
    for manifest in ROOT_MANIFESTS {
        hash_named_file(&mut hasher, b"manifest:", repo_root, manifest)?;
    }

    Ok(hasher.finalize().to_hex().to_string())
}

/// Looks up a previously stored Tier 1 result by `key`.
pub fn lookup(repo_root: &Path, key: &str) -> Result<Option<CachedTier1>> {
    let path = cache_path(repo_root);
    if !path.exists() {
        return Ok(None);
    }
    let store = load_store(&path)?;
    Ok(store.entries.into_iter().rev().find(|e| e.key == key))
}

/// Stores or refreshes a Tier 1 check entry, keeping at most the 64 most recent entries.
pub fn store(repo_root: &Path, entry: CachedTier1) -> Result<()> {
    let path = cache_path(repo_root);
    let lock_path = path.with_extension("json.lock");
    with_file_lock(&lock_path, LOCK_TIMEOUT_MS, || {
        let mut store = if path.exists() {
            load_store(&path)?
        } else {
            CheckCacheStore::default()
        };

        store.entries.retain(|existing| existing.key != entry.key);
        store.entries.push(entry);
        if store.entries.len() > MAX_CACHE_ENTRIES {
            let overflow = store.entries.len() - MAX_CACHE_ENTRIES;
            store.entries.drain(0..overflow);
        }

        let json = serde_json::to_string_pretty(&store)?;
        atomic_write(&path, format!("{json}\n").as_bytes())
    })
}

fn cache_path(repo_root: &Path) -> PathBuf {
    repo_root
        .join(".harness")
        .join(".state")
        .join("check-cache.json")
}

fn load_store(path: &Path) -> Result<CheckCacheStore> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn normalize_rel_path(raw: &str) -> String {
    let replaced = raw.trim().replace('\\', "/");
    replaced
        .strip_prefix("./")
        .unwrap_or(&replaced)
        .trim_start_matches('/')
        .to_owned()
}

fn hash_named_file(
    outer: &mut blake3::Hasher,
    prefix: &[u8],
    repo_root: &Path,
    rel: &str,
) -> Result<()> {
    outer.update(prefix);
    outer.update(rel.as_bytes());
    outer.update(b":");

    let full_path = repo_root.join(rel);
    if full_path.is_dir() {
        outer.update(b"<dir>");
        outer.update(b"\0");
        return Ok(());
    }
    match File::open(&full_path) {
        Ok(mut file) => {
            let mut file_hasher = blake3::Hasher::new();
            file_hasher
                .update_reader(&mut file)
                .with_context(|| format!("failed to hash {}", full_path.display()))?;
            let digest = file_hasher.finalize();
            outer.update(digest.as_bytes());
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {
            outer.update(b"<missing>");
        }
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to open {} for cache key", full_path.display()));
        }
    }

    outer.update(b"\0");
    Ok(())
}
