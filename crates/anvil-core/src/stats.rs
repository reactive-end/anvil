//! Lock-free JSONL telemetry appending with periodic compaction into `.harness/.state/stats.json`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::fs_util::{atomic_write, with_file_lock};

const LOCK_TIMEOUT_MS: u64 = 2_000;
const STATS_JSONL_COMPACT_BYTES: u64 = 64 * 1024;
const MAX_LATENCY_SAMPLES: usize = 200;

/// Cumulative observability metrics for hooks, check cache, compressor, Code Mode, and token guard.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnvilStats {
    pub hook_calls: u64,
    pub total_hook_ms: u64,
    pub check_cache_hits: u64,
    pub tokens_saved_compressor: u64,
    pub tokens_saved_symbol_read: u64,
    pub tokens_saved_outline: u64,
    pub code_mode_calls: u64,
    pub tokens_saved_code_mode: u64,
    pub token_guard_blocks: u64,
    pub token_guard_warnings: u64,
    pub hook_latencies_ms: Vec<u64>,
    pub rule_hits: FxHashMap<String, u64>,
}

/// Telemetry events recorded by `anvil` subsystems.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatsEvent {
    HookCall { elapsed_ms: u64 },
    CheckCacheHit,
    CompressorSaved { raw_bytes: usize, compact_bytes: usize },
    SymbolReadSaved { tokens_saved: u64 },
    OutlineSaved { tokens_saved: u64 },
    CodeModeExec { calls_executed: u64, tokens_saved: u64 },
    TokenGuard { blocked: bool },
    RuleHit { rule: String },
}

/// Computes an integer percentile (`p` in `0.0..=100.0`) from latency samples.
pub fn percentile_ms(samples: &[u64], pct: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let clamped = pct.clamp(0.0, 100.0) / 100.0;
    let max_idx = sorted.len().saturating_sub(1);
    let idx = ((max_idx as f64) * clamped).round() as usize;
    sorted[idx.min(max_idx)]
}

/// Estimates token count using `ceil(bytes / 4)`.
pub fn estimate_tokens(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(4)
}

/// Appends a single telemetry event to `.harness/.state/stats.jsonl` without advisory locking.
pub fn record_event(repo_root: &Path, event: StatsEvent) -> Result<()> {
    let path = stats_jsonl_path(repo_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut line = serde_json::to_string(&event)?;
    line.push('\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("failed to append {}", path.display()))
}

/// Loads accumulated stats by folding `stats.json` and `stats.jsonl`, compacting when needed.
pub fn load_stats(repo_root: &Path) -> Result<AnvilStats> {
    let path = stats_path(repo_root);
    let jsonl = stats_jsonl_path(repo_root);
    let mut stats = read_stats_or_default(&path)?;
    fold_jsonl(&jsonl, &mut stats);

    if fs::metadata(&jsonl)
        .map(|m| m.len() >= STATS_JSONL_COMPACT_BYTES)
        .unwrap_or(false)
    {
        let _ = compact_stats(repo_root);
    }
    Ok(stats)
}

/// Resets `.harness/.state/stats.json` and removes pending `.harness/.state/stats.jsonl`.
pub fn reset_stats(repo_root: &Path) -> Result<()> {
    let path = stats_path(repo_root);
    let lock_path = path.with_extension("json.lock");
    with_file_lock(&lock_path, LOCK_TIMEOUT_MS, || {
        let _ = fs::remove_file(stats_jsonl_path(repo_root));
        write_stats(&path, &AnvilStats::default())
    })
}

fn compact_stats(repo_root: &Path) -> Result<()> {
    let path = stats_path(repo_root);
    let jsonl = stats_jsonl_path(repo_root);
    let lock_path = path.with_extension("json.lock");
    with_file_lock(&lock_path, LOCK_TIMEOUT_MS, || {
        let mut stats = read_stats_or_default(&path)?;
        fold_jsonl(&jsonl, &mut stats);
        write_stats(&path, &stats)?;
        let _ = fs::write(&jsonl, b"");
        Ok(())
    })
}

fn fold_jsonl(jsonl_path: &Path, stats: &mut AnvilStats) {
    let Ok(raw) = fs::read_to_string(jsonl_path) else {
        return;
    };
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(ev) = serde_json::from_str::<StatsEvent>(trimmed) {
            apply_event(stats, ev);
        }
    }
}

fn stats_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".harness").join(".state").join("stats.json")
}

fn stats_jsonl_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".harness").join(".state").join("stats.jsonl")
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
            stats.hook_latencies_ms.push(elapsed_ms);
            if stats.hook_latencies_ms.len() > MAX_LATENCY_SAMPLES {
                let drop_n = stats.hook_latencies_ms.len() - MAX_LATENCY_SAMPLES;
                stats.hook_latencies_ms.drain(..drop_n);
            }
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
        StatsEvent::CodeModeExec {
            calls_executed: _,
            tokens_saved,
        } => {
            stats.code_mode_calls = stats.code_mode_calls.saturating_add(1);
            stats.tokens_saved_code_mode =
                stats.tokens_saved_code_mode.saturating_add(tokens_saved);
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
