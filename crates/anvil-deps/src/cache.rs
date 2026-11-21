//! Disk-backed TTL cache at `.harness/.state/deps-cache.json` with atomic writes.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
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

    /// Persists the cache atomically only if new entries were inserted.
    pub(crate) fn save_if_dirty(&self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;

        let tmp = parent.join(format!(
            ".deps-cache.json.tmp-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let json = serde_json::to_string(&self.data).context("failed to serialize deps cache")?;
        if let Err(err) = fs::write(&tmp, json.as_bytes()) {
            let _ = fs::remove_file(&tmp);
            return Err(err).with_context(|| format!("failed to write {}", tmp.display()));
        }
        if let Err(err) = fs::rename(&tmp, &self.path) {
            let _ = fs::remove_file(&tmp);
            return Err(err).with_context(|| format!("failed to replace {}", self.path.display()));
        }
        Ok(())
    }
}

fn cache_key(ecosystem: Ecosystem, package: &str) -> String {
    format!("{}:{package}", ecosystem.as_str())
}
