//! Persistent Circuit Breaker (N=3) with normalized error fingerprinting.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::fs_util::{atomic_write, with_file_lock};

const LOCK_TIMEOUT_MS: u64 = 2_000;

/// State of the circuit breaker persisted in `.harness/.state/circuit.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CircuitState {
    Closed,
    Open,
}

/// Structured escalation payload produced once consecutive identical failures reach `limit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EscalationSummary {
    pub fingerprint: String,
    pub tool: String,
    pub consecutive_failures: u32,
    pub normalized_error: String,
    pub last_command: String,
    pub opened_at: DateTime<Utc>,
    pub recommended_action: String,
}

/// Persisted circuit breaker snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CircuitStatus {
    pub state: CircuitState,
    pub limit: u32,
    pub consecutive_failures: u32,
    pub last_fingerprint: Option<String>,
    pub last_tool: Option<String>,
    #[serde(default)]
    pub last_command_key: Option<String>,
    pub recent_samples: Vec<String>,
    pub escalation: Option<EscalationSummary>,
    pub updated_at: DateTime<Utc>,
}

impl CircuitStatus {
    fn new(limit: u32) -> Self {
        Self {
            state: CircuitState::Closed,
            limit: limit.max(1),
            consecutive_failures: 0,
            last_fingerprint: None,
            last_tool: None,
            last_command_key: None,
            recent_samples: Vec::new(),
            escalation: None,
            updated_at: Utc::now(),
        }
    }

    /// Returns true when the circuit has tripped and blocks further blind attempts.
    pub fn is_open(&self) -> bool {
        self.state == CircuitState::Open
    }
}

/// Manages failure fingerprinting and state transitions in `.harness/.state/circuit.json`.
pub struct CircuitBreaker {
    state_path: PathBuf,
    limit: u32,
}

impl CircuitBreaker {
    pub fn new(repo_root: &Path, limit: u32) -> Self {
        Self {
            state_path: repo_root.join(".harness").join(".state").join("circuit.json"),
            limit: limit.max(1),
        }
    }

    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    fn lock_path(&self) -> PathBuf {
        self.state_path.with_extension("json.lock")
    }

    /// Reads current circuit status, returning a clean CLOSED snapshot if missing.
    pub fn status(&self) -> Result<CircuitStatus> {
        if !self.state_path.exists() {
            return Ok(CircuitStatus::new(self.limit));
        }
        let raw = fs::read_to_string(&self.state_path)
            .with_context(|| format!("failed to read {}", self.state_path.display()))?;
        let mut parsed: CircuitStatus = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", self.state_path.display()))?;
        parsed.limit = self.limit;
        Ok(parsed)
    }

    /// Records a tool failure; trips to `OPEN` on the Nth consecutive identical fingerprint.
    pub fn record_failure(
        &self,
        tool: &str,
        raw_error: &str,
        command: &str,
    ) -> Result<CircuitStatus> {
        with_file_lock(&self.lock_path(), LOCK_TIMEOUT_MS, || {
            let mut status = self.status()?;
            if status.is_open() {
                return Ok(status);
            }
            let normalized = normalize_error(raw_error);
            let fp = fingerprint_normalized(&normalized);
            let now = Utc::now();

            if status.last_fingerprint.as_deref() == Some(fp.as_str()) {
                status.consecutive_failures = status.consecutive_failures.saturating_add(1);
            } else {
                status.last_fingerprint = Some(fp.clone());
                status.consecutive_failures = 1;
                status.recent_samples.clear();
            }

            status.last_tool = Some(tool.to_owned());
            status.last_command_key = Some(command_family_key(command));
            push_bounded_sample(&mut status.recent_samples, raw_error, 5);
            status.updated_at = now;

            if status.consecutive_failures >= status.limit {
                status.state = CircuitState::Open;
                status.escalation = Some(EscalationSummary {
                    fingerprint: fp,
                    tool: tool.to_owned(),
                    consecutive_failures: status.consecutive_failures,
                    normalized_error: normalized,
                    last_command: command.to_owned(),
                    opened_at: now,
                    recommended_action:
                        "Stop automated retries, inspect root cause, and run `anvil circuit reset` after fixing."
                            .to_owned(),
                });
            }

            self.save(&status)?;
            Ok(status)
        })
    }

    /// Clears consecutive failure streak if circuit is still `CLOSED`.
    /// An `OPEN` circuit stays blocked until `reset()` is explicitly invoked.
    pub fn record_success(&self) -> Result<CircuitStatus> {
        with_file_lock(&self.lock_path(), LOCK_TIMEOUT_MS, || {
            let mut status = self.status()?;
            if status.is_open() {
                return Ok(status);
            }
            status.consecutive_failures = 0;
            status.last_fingerprint = None;
            status.last_command_key = None;
            status.recent_samples.clear();
            status.updated_at = Utc::now();
            self.save(&status)?;
            Ok(status)
        })
    }

    /// Clears consecutive failure streak only when `command` matches the failing command family.
    /// An `OPEN` circuit stays blocked until `reset()` is explicitly invoked.
    pub fn record_command_success(&self, command: &str) -> Result<CircuitStatus> {
        with_file_lock(&self.lock_path(), LOCK_TIMEOUT_MS, || {
            let mut status = self.status()?;
            if status.is_open() {
                return Ok(status);
            }
            let key = command_family_key(command);
            if status.last_command_key.as_deref() == Some(key.as_str()) {
                status.consecutive_failures = 0;
                status.last_fingerprint = None;
                status.last_tool = None;
                status.last_command_key = None;
                status.recent_samples.clear();
                status.updated_at = Utc::now();
                self.save(&status)?;
            }
            Ok(status)
        })
    }

    /// Explicitly resets the circuit breaker back to `CLOSED`.
    pub fn reset(&self) -> Result<CircuitStatus> {
        with_file_lock(&self.lock_path(), LOCK_TIMEOUT_MS, || {
            let status = CircuitStatus::new(self.limit);
            self.save(&status)?;
            Ok(status)
        })
    }

    fn save(&self, status: &CircuitStatus) -> Result<()> {
        let json = serde_json::to_string_pretty(status)?;
        atomic_write(&self.state_path, format!("{json}\n").as_bytes())
    }
}

/// Normalizes volatile timestamps, durations, hex/temp IDs, line/column numbers, and paths.
pub fn normalize_error(raw: &str) -> String {
    static RE_ISO_TS: OnceLock<Regex> = OnceLock::new();
    static RE_UUID: OnceLock<Regex> = OnceLock::new();
    static RE_TMP: OnceLock<Regex> = OnceLock::new();
    static RE_HEX_ADDR: OnceLock<Regex> = OnceLock::new();
    static RE_HEX_LONG: OnceLock<Regex> = OnceLock::new();
    static RE_LINE_COL: OnceLock<Regex> = OnceLock::new();
    static RE_DURATION: OnceLock<Regex> = OnceLock::new();
    static RE_WS: OnceLock<Regex> = OnceLock::new();

    let re_ts = RE_ISO_TS.get_or_init(|| {
        Regex::new(
            r"\b\d{4}-\d{2}-\d{2}(?:[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?)?\b",
        )
        .expect("valid iso timestamp regex")
    });
    let re_uuid = RE_UUID.get_or_init(|| {
        Regex::new(
            r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b",
        )
        .expect("valid uuid regex")
    });
    let re_tmp = RE_TMP.get_or_init(|| {
        Regex::new(r"(?:/tmp/[A-Za-z0-9._-]+|\.tmp\.[A-Za-z0-9._-]+)").expect("valid temp id regex")
    });
    let re_addr =
        RE_HEX_ADDR.get_or_init(|| Regex::new(r"0x[0-9a-fA-F]+").expect("valid hex addr regex"));
    let re_hex_long = RE_HEX_LONG
        .get_or_init(|| Regex::new(r"\b[0-9a-fA-F]{12,}\b").expect("valid long hex regex"));
    let re_loc = RE_LINE_COL
        .get_or_init(|| Regex::new(r"(?::|\()\d+(?::|,)\d+\)?:?").expect("valid location regex"));
    let re_dur = RE_DURATION.get_or_init(|| {
        Regex::new(r"\b\d+(?:\.\d+)?(?:ms|µs|us|ns|s)\b").expect("valid duration regex")
    });
    let re_ws = RE_WS.get_or_init(|| Regex::new(r"\s+").expect("valid whitespace regex"));

    let unified = raw.replace('\\', "/");
    let s1 = re_ts.replace_all(&unified, "<ts>");
    let s2 = re_uuid.replace_all(&s1, "<id>");
    let s3 = re_tmp.replace_all(&s2, "<tmp>");
    let s4 = re_addr.replace_all(&s3, "0x<addr>");
    let s5 = re_hex_long.replace_all(&s4, "<hex>");
    let s6 = re_loc.replace_all(&s5, ":<loc>");
    let s7 = re_dur.replace_all(&s6, "<dur>");
    re_ws.replace_all(s7.trim(), " ").into_owned()
}

/// Computes a deterministic `blake3` hex fingerprint directly over raw error text.
pub fn fingerprint(raw: &str) -> String {
    fingerprint_normalized(&normalize_error(raw))
}

/// Computes a deterministic `blake3` hex fingerprint over normalized error text.
pub fn fingerprint_normalized(normalized: &str) -> String {
    blake3::hash(normalized.as_bytes()).to_hex().to_string()
}

fn push_bounded_sample(samples: &mut Vec<String>, raw: &str, max_samples: usize) {
    let trimmed = raw.trim();
    let snippet: String = trimmed.chars().take(240).collect();
    if samples.len() >= max_samples {
        samples.remove(0);
    }
    samples.push(snippet);
}

const MULTI_SUBCOMMAND_RUNNERS: &[&str] = &[
    "cargo", "npm", "pnpm", "yarn", "bun", "git", "php", "python", "python3", "uv", "poetry",
    "flutter", "dart", "gradle", "gradlew", "anvil", "composer",
];

/// Derives a normalized command-family key (e.g. `"cargo test"`, `"vitest"`, `"git status"`).
pub fn command_family_key(command: &str) -> String {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let mut idx = 0usize;
    while idx < tokens.len() && is_env_assignment(tokens[idx]) {
        idx += 1;
    }
    let Some(first) = tokens.get(idx) else {
        return String::new();
    };
    let mut prog = normalize_prog_token(first);
    idx += 1;

    if matches!(prog.as_str(), "npx" | "bunx") {
        let mut next_idx = idx;
        while next_idx < tokens.len() && tokens[next_idx].starts_with('-') {
            next_idx += 1;
        }
        if let Some(wrapped) = tokens.get(next_idx) {
            prog = normalize_prog_token(wrapped);
            idx = next_idx + 1;
        }
    }

    if MULTI_SUBCOMMAND_RUNNERS.contains(&prog.as_str())
        && let Some(sub) = find_subcommand(&tokens[idx..])
    {
        return format!("{prog} {}", sub.to_ascii_lowercase());
    }
    prog
}

fn find_subcommand<'a>(args: &[&'a str]) -> Option<&'a str> {
    const VALUE_FLAGS: &[&str] = &[
        "-p",
        "--package",
        "--prefix",
        "-C",
        "--manifest-path",
        "-f",
        "--file",
        "--root",
        "--repo",
    ];
    let mut i = 0usize;
    while i < args.len() {
        let tok = args[i];
        if VALUE_FLAGS.contains(&tok) {
            i += 2;
            continue;
        }
        if tok.starts_with('-') {
            i += 1;
            continue;
        }
        return Some(tok);
    }
    None
}

fn is_env_assignment(token: &str) -> bool {
    token.split_once('=').is_some_and(|(key, _)| {
        !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn normalize_prog_token(token: &str) -> String {
    let base = token.rsplit(['/', '\\']).next().unwrap_or(token);
    let lower = base.to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat"] {
        if let Some(stripped) = lower.strip_suffix(ext) {
            return stripped.to_owned();
        }
    }
    lower
}
