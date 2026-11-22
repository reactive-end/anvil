//! Local telemetry & token-savings store persisted at `.harness/.state/stats.json`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::fs_util::{atomic_write, with_file_lock};

const LOCK_TIMEOUT_MS: u64 = 2_000;

/// Cumulative observability metrics for hooks, check cache, compressor, and token guard.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnvilStats {
    pub hook_calls: u64,
    pub total_hook_ms: u64,
    pub check_cache_hits: u64,
    pub tokens_saved_compressor: u64,
    pub tokens_saved_symbol_read: u64,
    pub tokens_saved_outline: u64,
    pub token_guard_blocks: u64,
    pub token_guard_warnings: u64,
    pub rule_hits: FxHashMap<String, u64>,
}

/// Telemetry events recorded by `anvil` subsystems.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatsEvent {
    HookCall { elapsed_ms: u64 },
    CheckCacheHit,
    CompressorSaved { raw_bytes: usize, compact_bytes: usize },
    SymbolReadSaved { tokens_saved: u64 },
    OutlineSaved { tokens_saved: u64 },
    TokenGuard { blocked: bool },
    RuleHit { rule: String },
}

/// Estimates token count using `ceil(bytes / 4)`.
pub fn estimate_tokens(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(4)
}

/// Records a single telemetry event into `.harness/.state/stats.json` under a file lock.
pub fn record_event(repo_root: &Path, event: StatsEvent) -> Result<()> {
    let path = stats_path(repo_root);
    let lock_path = path.with_extension("json.lock");
    with_file_lock(&lock_path, LOCK_TIMEOUT_MS, || {
        let mut stats = read_stats_or_default(&path)?;
        apply_event(&mut stats, event);
        write_stats(&path, &stats)
    })
}

/// Loads current telemetry stats from `.harness/.state/stats.json`, or defaults if missing.
pub fn load_stats(repo_root: &Path) -> Result<AnvilStats> {
    read_stats_or_default(&stats_path(repo_root))
}

/// Resets `.harness/.state/stats.json` back to zeroed defaults under a file lock.
pub fn reset_stats(repo_root: &Path) -> Result<()> {
    let path = stats_path(repo_root);
    let lock_path = path.with_extension("json.lock");
    with_file_lock(&lock_path, LOCK_TIMEOUT_MS, || {
        write_stats(&path, &AnvilStats::default())
    })
}

fn stats_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".harness").join(".state").join("stats.json")
}

fn read_stats_or_default(path: &Path) -> Result<AnvilStats> {
    if !path.exists() {
        return Ok(AnvilStats::default());
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn write_stats(path: &Path, stats: &AnvilStats) -> Result<()> {
    let json = serde_json::to_string_pretty(stats)?;
    atomic_write(path, format!("{json}\n").as_bytes())
}

fn apply_event(stats: &mut AnvilStats, event: StatsEvent) {
    match event {
        StatsEvent::HookCall { elapsed_ms } => {
            stats.hook_calls = stats.hook_calls.saturating_add(1);
            stats.total_hook_ms = stats.total_hook_ms.saturating_add(elapsed_ms);
        }
        StatsEvent::CheckCacheHit => {
            stats.check_cache_hits = stats.check_cache_hits.saturating_add(1);
        }
        StatsEvent::CompressorSaved {
            raw_bytes,
            compact_bytes,
        } => {
            let saved_tokens = estimate_tokens(raw_bytes.saturating_sub(compact_bytes));
            stats.tokens_saved_compressor =
                stats.tokens_saved_compressor.saturating_add(saved_tokens);
        }
        StatsEvent::SymbolReadSaved { tokens_saved } => {
            stats.tokens_saved_symbol_read =
                stats.tokens_saved_symbol_read.saturating_add(tokens_saved);
        }
        StatsEvent::OutlineSaved { tokens_saved } => {
            stats.tokens_saved_outline = stats.tokens_saved_outline.saturating_add(tokens_saved);
        }
        StatsEvent::TokenGuard { blocked } => {
            if blocked {
                stats.token_guard_blocks = stats.token_guard_blocks.saturating_add(1);
            } else {
                stats.token_guard_warnings = stats.token_guard_warnings.saturating_add(1);
            }
        }
        StatsEvent::RuleHit { rule } => {
            let counter = stats.rule_hits.entry(rule).or_insert(0);
            *counter = counter.saturating_add(1);
        }
    }
}
