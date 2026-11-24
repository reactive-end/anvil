//! Metadata-keyed Tier 0 clean-file cache persisted at `.harness/.state/tier0-cache.json`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::AnvilConfig;
use crate::fs_util::{atomic_write, with_file_lock};

const MAX_TIER0_ENTRIES: usize = 512;
const LOCK_TIMEOUT_MS: u64 = 1_500;

/// Cached metadata signature proving a file passed Tier 0 checks for a specific ruleset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tier0CacheEntry {
    pub mtime_nanos: u128,
    pub len: u64,
    pub rules_hash: String,
    pub passed: bool,
}

/// Persisted Tier 0 clean-file cache.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Tier0Cache {
    #[serde(default)]
    pub entries: BTreeMap<String, Tier0CacheEntry>,
}

impl Tier0Cache {
    /// Loads `.harness/.state/tier0-cache.json` or returns an empty cache.
    pub fn load(repo_root: &Path) -> Self {
        let path = cache_path(repo_root);
        fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Self>(&raw).ok())
            .unwrap_or_default()
    }

    /// Returns `true` when `abs_path` matches the cached `(mtime, len)` and `rules_hash` cleanly.
    pub fn is_clean_hit(&self, abs_path: &Path, rel: &str, rules_hash: &str) -> bool {
        let norm = rel.replace('\\', "/");
        let Some(entry) = self.entries.get(&norm) else {
            return false;
        };
        if !entry.passed || entry.rules_hash != rules_hash {
            return false;
        }
        file_meta_sig(abs_path)
            .is_some_and(|(mtime, len)| entry.mtime_nanos == mtime && entry.len == len)
    }

    /// Inserts or refreshes a clean check entry for `abs_path`.
    pub fn record_clean(&mut self, abs_path: &Path, rel: &str, rules_hash: &str) {
        let Some((mtime_nanos, len)) = file_meta_sig(abs_path) else {
            return;
        };
        let norm = rel.replace('\\', "/");
        self.entries.insert(
            norm,
            Tier0CacheEntry {
                mtime_nanos,
                len,
                rules_hash: rules_hash.to_owned(),
                passed: true,
            },
        );
    }

    /// Invalidates any cached entry for `rel` when errors remain.
    pub fn invalidate(&mut self, rel: &str) {
        let norm = rel.replace('\\', "/");
        self.entries.remove(&norm);
    }

    /// Persists the bounded cache atomically under advisory file lock.
    pub fn save(&mut self, repo_root: &Path) -> Result<()> {
        while self.entries.len() > MAX_TIER0_ENTRIES {
            let Some(first_key) = self.entries.keys().next().cloned() else {
                break;
            };
            self.entries.remove(&first_key);
        }
        let path = cache_path(repo_root);
        let lock_path = path.with_extension("json.lock");
        with_file_lock(&lock_path, LOCK_TIMEOUT_MS, || {
            let json = serde_json::to_string(&self)?;
            atomic_write(&path, json.as_bytes())
        })
    }
}

/// Hashes `config.allowed_escapes` and all `.harness/rules/*.json` files deterministically.
pub fn compute_rules_hash(repo_root: &Path, config: &AnvilConfig) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"anvil-tier0-rules-v1\0");
    for esc in &config.allowed_escapes {
        hasher.update(esc.as_bytes());
        hasher.update(b"\0");
    }
    let rules_dir = repo_root.join(".harness").join("rules");
    if let Ok(entries) = fs::read_dir(&rules_dir) {
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        files.sort();
        for file in files {
            if let Some(name) = file.file_name().and_then(|n| n.to_str()) {
                hasher.update(name.as_bytes());
            }
            if let Ok(bytes) = fs::read(&file) {
                hasher.update(&bytes);
            }
            hasher.update(b"\0");
        }
    }
    hasher.finalize().to_hex().to_string()
}

fn cache_path(repo_root: &Path) -> PathBuf {
    repo_root
        .join(".harness")
        .join(".state")
        .join("tier0-cache.json")
}

fn file_meta_sig(path: &Path) -> Option<(u128, u64)> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let nanos = modified.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some((nanos, meta.len()))
}
