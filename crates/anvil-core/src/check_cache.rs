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
const MANIFEST_FILES: [&str; 14] = [
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "tsconfig.json",
    "tsconfig.app.json",
    "tsconfig.node.json",
    "composer.json",
    "phpstan.neon",
    "phpstan.neon.dist",
    "psalm.xml",
    "pubspec.yaml",
    "build.gradle.kts",
    "build.gradle",
    "project.godot",
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
/// `scope_files`, root manifests, `.harness/rules/*.json`, and `extra_salt`.
pub fn compute_scope_key(
    repo_root: &Path,
    scope_files: &[String],
    extra_salt: &str,
) -> Result<String> {
    compute_scope_key_with_subrepos(repo_root, scope_files, &[], extra_salt)
}

/// Computes a deterministic `blake3` hex digest over sorted `(rel_path, blake3_bytes)` for
/// `scope_files`, root manifests, subrepo manifests, promoted ratchet rules, and `extra_salt`.
pub fn compute_scope_key_with_subrepos(
    repo_root: &Path,
    scope_files: &[String],
    subrepo_paths: &[String],
    extra_salt: &str,
) -> Result<String> {
    let normalized = normalize_set(scope_files);
    let subrepos = normalize_subrepos(subrepo_paths);

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"anvil-tier1-cache-v1\0salt:");
    hasher.update(extra_salt.as_bytes());
    hasher.update(b"\0");

    for rel in &normalized {
        hash_named_file(&mut hasher, b"file:", repo_root, rel)?;
    }
    for manifest in MANIFEST_FILES {
        hash_named_file(&mut hasher, b"manifest:", repo_root, manifest)?;
    }
    for sub in &subrepos {
        for manifest in MANIFEST_FILES {
            let sub_manifest = format!("{sub}/{manifest}");
            hash_named_file(&mut hasher, b"submanifest:", repo_root, &sub_manifest)?;
        }
    }
    hash_ratchet_rules(&mut hasher, repo_root)?;

    Ok(hasher.finalize().to_hex().to_string())
}

fn normalize_set(paths: &[String]) -> BTreeSet<String> {
    paths.iter().map(|raw| normalize_rel_path(raw)).filter(|norm| !norm.is_empty()).collect()
}

fn normalize_subrepos(subrepo_paths: &[String]) -> BTreeSet<String> {
    subrepo_paths
        .iter()
        .map(|raw| normalize_rel_path(raw).trim_end_matches('/').to_owned())
        .filter(|norm| !norm.is_empty() && norm != ".")
        .collect()
}

fn hash_ratchet_rules(hasher: &mut blake3::Hasher, repo_root: &Path) -> Result<()> {
    let rules_dir = repo_root.join(".harness").join("rules");
    if !rules_dir.is_dir() {
        return Ok(());
    }
    let mut names: Vec<String> = fs::read_dir(&rules_dir)
        .with_context(|| format!("failed to read {}", rules_dir.display()))?
        .flatten()
        .filter(|entry| {
            let path = entry.path();
            entry.file_type().is_ok_and(|ft| ft.is_file())
                && path.extension().and_then(|e| e.to_str()) == Some("json")
        })
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    for name in names {
        let rel = format!(".harness/rules/{name}");
        hash_named_file(hasher, b"rule:", repo_root, &rel)?;
    }
    Ok(())
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
        let mut store = if path.exists() { load_store(&path)? } else { CheckCacheStore::default() };

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
    repo_root.join(".harness").join(".state").join("check-cache.json")
}

fn load_store(path: &Path) -> Result<CheckCacheStore> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn normalize_rel_path(raw: &str) -> String {
    let replaced = raw.trim().replace('\\', "/");
    replaced.strip_prefix("./").unwrap_or(&replaced).trim_start_matches('/').to_owned()
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
