//! Escape rate: review findings recorded after `anvil check` was green, per category.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use super::{StatsEvent, load_stats, record_event};

/// Findings after green needed before a category is suggested as a Ratchet rule.
pub const RATCHET_HINT_THRESHOLD: u64 = 3;
/// Accepted `--severity` values, most severe first.
pub const SEVERITIES: [&str; 3] = ["high", "medium", "low"];
/// Accepted `--source` values.
pub const SOURCES: [&str; 4] = ["orchestrator", "human", "ci", "tool"];
const MAX_HASHED_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Findings recorded for one category.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CategoryStats {
    pub findings: u64,
    pub after_green: u64,
}

/// Check outcomes and review findings folded from `stats.jsonl`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QualityStats {
    pub checks_passed: u64,
    pub checks_failed: u64,
    pub last_check_passed: Option<bool>,
    pub last_check_scope: Option<String>,
    pub findings: u64,
    pub findings_after_green: u64,
    pub categories: FxHashMap<String, CategoryStats>,
}

impl QualityStats {
    /// Folds a `CheckOutcome` or `ReviewFinding` event; other events are ignored.
    pub(super) fn apply(&mut self, event: &StatsEvent) {
        match event {
            StatsEvent::CheckOutcome { passed, scope_key, .. } => self.record_check(*passed, scope_key),
            StatsEvent::ReviewFinding { category, after_green, .. } => self.record_finding(category, *after_green),
            _ => {}
        }
    }

    fn record_check(&mut self, passed: bool, scope_key: &str) {
        if passed {
            self.checks_passed = self.checks_passed.saturating_add(1);
        } else {
            self.checks_failed = self.checks_failed.saturating_add(1);
        }
        self.last_check_passed = Some(passed);
        self.last_check_scope = Some(scope_key.to_owned());
    }

    fn record_finding(&mut self, category: &str, after_green: bool) {
        self.findings = self.findings.saturating_add(1);
        let entry = self.categories.entry(category.to_owned()).or_default();
        entry.findings = entry.findings.saturating_add(1);
        if after_green {
            self.findings_after_green = self.findings_after_green.saturating_add(1);
            entry.after_green = entry.after_green.saturating_add(1);
        }
    }

    /// `true` when no check or finding was ever recorded.
    pub fn is_empty(&self) -> bool {
        self.checks_passed == 0 && self.checks_failed == 0 && self.findings == 0
    }

    /// Findings after green divided by green checks; `None` before the first green check.
    pub fn escape_rate(&self) -> Option<f64> {
        (self.checks_passed > 0).then(|| self.findings_after_green as f64 / self.checks_passed as f64)
    }

    /// Categories with findings after green, most frequent first.
    pub fn top_categories(&self, limit: usize) -> Vec<(&str, u64)> {
        let mut ranked: Vec<(&str, u64)> = self
            .categories
            .iter()
            .filter(|(_, c)| c.after_green > 0)
            .map(|(name, c)| (name.as_str(), c.after_green))
            .collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        ranked.truncate(limit);
        ranked
    }

    /// Categories frequent enough after green to become deterministic rules.
    pub fn ratchet_candidates(&self) -> Vec<(&str, u64)> {
        let mut ranked = self.top_categories(usize::MAX);
        ranked.retain(|(_, n)| *n >= RATCHET_HINT_THRESHOLD);
        ranked
    }
}

/// A finding as recorded by `anvil review log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewEntry {
    pub at: DateTime<Utc>,
    pub description: String,
    pub category: String,
    pub severity: String,
    pub source: String,
    pub file: Option<String>,
    pub after_green: bool,
}

/// Fields of a new finding.
#[derive(Debug, Clone)]
pub struct NewFinding {
    pub description: String,
    pub category: String,
    pub severity: String,
    pub source: String,
    pub file: Option<String>,
}

/// The stored finding and the categories known before it.
#[derive(Debug, Clone)]
pub struct LoggedReview {
    pub entry: ReviewEntry,
    pub new_category: bool,
    pub known_categories: Vec<String>,
}

/// Categories are kebab-case slugs (`error-handling`, `dead-code`).
pub fn validate_category(category: &str) -> Result<()> {
    let slug = !category.is_empty()
        && category.split('-').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        });
    if !slug {
        bail!("category `{category}` must be a kebab-case slug such as `error-handling`");
    }
    Ok(())
}

fn validate_choice(name: &str, value: &str, allowed: &[&str]) -> Result<()> {
    if !allowed.contains(&value) {
        bail!("{name} `{value}` must be one of: {}", allowed.join(", "));
    }
    Ok(())
}

/// Records the outcome of one `anvil check` run against the content-addressed `scope_key` of its diff.
pub fn record_check_outcome(repo_root: &Path, passed: bool, tier: &str, changed: &[String]) -> Result<()> {
    let scope_key = scope_key(repo_root, changed);
    record_event(repo_root, StatsEvent::CheckOutcome { passed, tier: tier.to_owned(), scope_key })
}

/// Hash of the changed paths and their current contents (missing files hash as deleted).
pub fn scope_key(repo_root: &Path, changed: &[String]) -> String {
    let mut paths: Vec<&String> = changed.iter().collect();
    paths.sort();
    let mut hasher = blake3::Hasher::new();
    for rel in paths {
        hasher.update(rel.as_bytes());
        hasher.update(&[0]);
        let path = repo_root.join(rel);
        match fs::metadata(&path) {
            Ok(meta) if meta.is_file() && meta.len() <= MAX_HASHED_FILE_BYTES => {
                hasher.update(&fs::read(&path).unwrap_or_default());
            }
            Ok(_) => {}
            Err(_) => {
                hasher.update(b"<deleted>");
            }
        }
        hasher.update(&[0]);
    }
    hasher.finalize().to_hex()[..32].to_owned()
}

/// Appends a finding to the review log and the stats stream. `after_green` is whether the most recent
/// recorded `anvil check` passed.
pub fn log_review(repo_root: &Path, new: NewFinding) -> Result<LoggedReview> {
    validate_category(&new.category)?;
    validate_choice("severity", &new.severity, &SEVERITIES)?;
    validate_choice("source", &new.source, &SOURCES)?;
    if new.description.trim().is_empty() {
        bail!("a finding needs a description");
    }
    let quality = load_stats(repo_root)?.quality;
    let mut known_categories: Vec<String> = quality.categories.keys().cloned().collect();
    known_categories.sort();
    let new_category = !known_categories.contains(&new.category);
    let entry = ReviewEntry {
        at: Utc::now(),
        description: new.description,
        category: new.category,
        severity: new.severity,
        source: new.source,
        file: new.file,
        after_green: quality.last_check_passed == Some(true),
    };
    append_entry(&review_log_path(repo_root), &entry)?;
    record_event(
        repo_root,
        StatsEvent::ReviewFinding {
            category: entry.category.clone(),
            severity: entry.severity.clone(),
            source: entry.source.clone(),
            file: entry.file.clone(),
            after_green: entry.after_green,
        },
    )?;
    Ok(LoggedReview { entry, new_category, known_categories })
}

/// Recorded findings, oldest first, optionally limited to the last `since_days` days.
pub fn list_reviews(repo_root: &Path, since_days: Option<u32>) -> Result<Vec<ReviewEntry>> {
    let Ok(raw) = fs::read_to_string(review_log_path(repo_root)) else {
        return Ok(Vec::new());
    };
    let cutoff = since_days.map(|days| Utc::now() - Duration::days(i64::from(days)));
    Ok(raw
        .lines()
        .filter_map(|line| serde_json::from_str::<ReviewEntry>(line).ok())
        .filter(|entry| cutoff.is_none_or(|limit| entry.at >= limit))
        .collect())
}

fn review_log_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".harness").join(".state").join("review-log.jsonl")
}

fn append_entry(path: &Path, entry: &ReviewEntry) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut line = serde_json::to_string(entry)?;
    line.push('\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(line.as_bytes()).with_context(|| format!("failed to append {}", path.display()))
}
