//! Disk-backed TTL cache at `.harness/.state/deps-cache.json` with atomic writes.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use anvil_core::fs_util::atomic_write;
use chrono::{DateTime, Utc};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::model::{Ecosystem, PackageHealthInfo};

const CACHE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedPackageEntry {
    info: PackageHealthInfo,
    cached_at_secs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DepsCacheDisk {
    version: u32,
    entries: FxHashMap<String, CachedPackageEntry>,
}

impl Default for DepsCacheDisk {
    fn default() -> Self {
        Self {
            version: CACHE_SCHEMA_VERSION,
            entries: FxHashMap::default(),
        }
    }
}

/// In-memory view of `.harness/.state/deps-cache.json` that only writes on mutation.
pub(crate) struct DepsCache {
    path: PathBuf,
    data: DepsCacheDisk,
    dirty: bool,
}

impl DepsCache {
    /// Returns `.harness/.state/deps-cache.json`.
    pub(crate) fn cache_path(repo_root: &Path) -> PathBuf {
        repo_root
            .join(".harness")
            .join(".state")
            .join("deps-cache.json")
    }

    /// Loads cache from disk if present and valid; falls back to an empty cache on corruption.
    pub(crate) fn load(repo_root: &Path) -> Self {
        let path = Self::cache_path(repo_root);
        let data = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<DepsCacheDisk>(&bytes).ok())
            .filter(|disk| disk.version == CACHE_SCHEMA_VERSION)
            .unwrap_or_default();
        Self {
            path,
            data,
            dirty: false,
        }
    }

    /// Returns cached package metadata when present and younger than `ttl_hours`.
    pub(crate) fn get_fresh(
        &self,
        ecosystem: Ecosystem,
        package: &str,
        ttl_hours: i64,
        now: DateTime<Utc>,
    ) -> Option<&PackageHealthInfo> {
        let key = cache_key(ecosystem, package);
        let entry = self.data.entries.get(&key)?;
        let ttl_secs = ttl_hours.saturating_mul(3600);
        if now.timestamp() - entry.cached_at_secs <= ttl_secs {
            Some(&entry.info)
        } else {
            None
        }
    }

    /// Inserts or refreshes a registry entry and marks the cache dirty.
    pub(crate) fn insert(&mut self, info: PackageHealthInfo, now: DateTime<Utc>) {
        let key = cache_key(info.ecosystem, &info.name);
        self.data.entries.insert(
            key,
            CachedPackageEntry {
                info,
                cached_at_secs: now.timestamp(),
            },
        );
        self.dirty = true;
    }

    /// Removes cache entries older than `ttl_hours` and marks the cache dirty if any were pruned.
    pub(crate) fn prune_expired(&mut self, ttl_hours: i64, now: DateTime<Utc>) {
        let ttl_secs = ttl_hours.saturating_mul(3600);
        let before = self.data.entries.len();
        self.data
            .entries
            .retain(|_, entry| now.timestamp() - entry.cached_at_secs <= ttl_secs);
        if self.data.entries.len() != before {
            self.dirty = true;
        }
    }

    /// Persists the cache atomically only if new entries were inserted or pruned.
    pub(crate) fn save_if_dirty(&self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let json = serde_json::to_string(&self.data).context("failed to serialize deps cache")?;
        atomic_write(&self.path, json.as_bytes())
    }
}

fn cache_key(ecosystem: Ecosystem, package: &str) -> String {
    format!("{}:{package}", ecosystem.as_str())
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn prune_expired_drops_stale_entries_and_keeps_fresh() {
        let tmp = TempDir::new().expect("tempdir");
        let now = Utc::now();
        let mut cache = DepsCache::load(tmp.path());
        let mk = |name: &str| PackageHealthInfo {
            ecosystem: Ecosystem::Npm,
            name: name.to_owned(),
            latest_version: Some("1.0.0".to_owned()),
            last_publish: None,
            deprecated: false,
            deprecation_message: None,
            archived: false,
            repository_url: None,
            vulnerabilities: Vec::new(),
        };
        cache.insert(mk("stale-pkg"), now - Duration::hours(30));
        cache.insert(mk("fresh-pkg"), now - Duration::hours(1));
        cache.save_if_dirty().expect("save");

        let mut reloaded = DepsCache::load(tmp.path());
        reloaded.prune_expired(24, now);
        assert!(reloaded.dirty);
        reloaded.save_if_dirty().expect("save pruned");

        let final_cache = DepsCache::load(tmp.path());
        assert!(final_cache.get_fresh(Ecosystem::Npm, "fresh-pkg", 24, now).is_some());
        assert!(!final_cache.data.entries.contains_key("npm:stale-pkg"));
    }
}
