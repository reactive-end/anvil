//! Governed linter ratchet: candidate logging, triple-filter evaluation, and rule promotion.

pub mod eval;
mod matcher;
pub mod types;

pub use eval::{
    CandidateTestMatch, CandidateTestReport, evaluate_promoted_rules, load_promoted_rules,
    test_candidate,
};
pub use types::{
    CandidateInput, PromotedRule, RatchetCandidate, RatchetEvaluation, RuleCandidate,
};

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use regex::Regex;
use serde::Deserialize;
use ulid::Ulid;

use crate::fs_util::atomic_write;

pub(crate) const MAX_RUNTIME_MS: u64 = 200;

pub fn candidates_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".harness").join("candidates")
}

pub fn rules_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".harness").join("rules")
}

/// Logs a candidate mistake; increments recurrence if `rule_name` already exists.
pub fn log_candidate(repo_root: &Path, input: CandidateInput) -> Result<RatchetCandidate> {
    let dir = candidates_dir(repo_root);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let existing = list_candidates(repo_root)?;
    let now = Utc::now();
    if let Some(mut found) = existing.into_iter().find(|c| c.rule_name == input.rule_name) {
        found.recurrence = found.recurrence.saturating_add(1);
        found.high_impact |= input.high_impact;
        found.updated_at = now;
        if let Some(ex) = input.example
            && !found.examples.contains(&ex)
            && found.examples.len() < 10
        {
            found.examples.push(ex);
        }
        save_candidate(&dir, &found)?;
        return Ok(found);
    }

    let pat_trimmed = input.pattern.trim();
    let candidate = RatchetCandidate {
        id: Ulid::new().to_string(),
        rule_name: input.rule_name,
        description: input.description,
        pattern: (!pat_trimmed.is_empty()).then(|| pat_trimmed.to_owned()),
        languages: input.languages,
        file_globs: Vec::new(),
        severity: None,
        recurrence: 1,
        high_impact: input.high_impact,
        deterministic: input.deterministic,
        estimated_runtime_ms: input.estimated_runtime_ms,
        fp_checked: false,
        false_positives_found: 0,
        examples: input.example.into_iter().collect(),
        created_at: now,
        updated_at: now,
    };
    save_candidate(&dir, &candidate)?;
    Ok(candidate)
}

/// Lists all candidate rules sorted by creation timestamp then ID.
pub fn list_candidates(repo_root: &Path) -> Result<Vec<RatchetCandidate>> {
    read_json_dir(&candidates_dir(repo_root))
}

/// Lists all promoted rules sorted by promotion timestamp then ID.
pub fn list_rules(repo_root: &Path) -> Result<Vec<PromotedRule>> {
    read_json_dir(&rules_dir(repo_root))
}

/// Evaluates the triple filter; optionally runs regex false-positive scan over `clean_samples`.
pub fn evaluate_candidate(
    repo_root: &Path,
    id: &str,
    clean_samples: Option<&[&str]>,
) -> Result<RatchetEvaluation> {
    let mut candidate = load_candidate(repo_root, id)?;
    let mut measured_ms = candidate.estimated_runtime_ms;
    let mut reasons = Vec::new();

    if let Some(samples) = clean_samples {
        let pat = candidate
            .pattern
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .with_context(|| format!("candidate `{}` has no regex pattern", candidate.id))?;
        let re = Regex::new(pat)
            .with_context(|| format!("invalid candidate regex pattern for `{}`", candidate.id))?;
        let start = Instant::now();
        let fps = samples.iter().filter(|s| re.is_match(s)).count();
        measured_ms = start.elapsed().as_millis() as u64;
        candidate.fp_checked = true;
        candidate.false_positives_found = fps;
        candidate.estimated_runtime_ms = candidate.estimated_runtime_ms.max(measured_ms);
        candidate.updated_at = Utc::now();
        save_candidate(&candidates_dir(repo_root), &candidate)?;
    }

    let f1 = candidate.recurrence >= 2 || candidate.high_impact;
    if !f1 {
        reasons.push("recurrence < 2 and not marked high_impact".to_owned());
    }
    let f2 = candidate.deterministic && measured_ms < MAX_RUNTIME_MS;
    if !f2 {
        reasons.push(format!("must be deterministic and < {MAX_RUNTIME_MS}ms ({measured_ms}ms)"));
    }
    let f3 = candidate.fp_checked && candidate.false_positives_found == 0;
    if !f3 {
        reasons.push(format!(
            "false-positive check incomplete or failed (checked={}, fps={})",
            candidate.fp_checked, candidate.false_positives_found
        ));
    }

    Ok(RatchetEvaluation {
        id: candidate.id,
        eligible: f1 && f2 && f3,
        filter_recurrence_or_impact: f1,
        filter_deterministic_fast: f2,
        filter_zero_false_positives: f3,
        measured_runtime_ms: measured_ms,
        reasons,
    })
}

/// Promotes an eligible candidate into `.harness/rules/<id>.json` and deletes the candidate file.
pub fn promote_candidate(repo_root: &Path, id: &str) -> Result<PromotedRule> {
    let eval = evaluate_candidate(repo_root, id, None)?;
    if !eval.eligible {
        bail!("candidate `{id}` failed ratchet triple filter: {}", eval.reasons.join("; "));
    }

    let candidate = load_candidate(repo_root, id)?;
    let rule = PromotedRule {
        id: candidate.id.clone(),
        rule_name: candidate.rule_name,
        description: candidate.description,
        pattern: candidate.pattern,
        languages: candidate.languages,
        file_globs: candidate.file_globs,
        severity: candidate.severity,
        recurrence_at_promotion: candidate.recurrence,
        promoted_at: Utc::now(),
    };

    let rule_path = rules_dir(repo_root).join(format!("{}.json", rule.id));
    let json = serde_json::to_string_pretty(&rule)?;
    atomic_write(&rule_path, format!("{json}\n").as_bytes())?;

    let cand_path = candidates_dir(repo_root).join(format!("{id}.json"));
    if cand_path.exists() {
        fs::remove_file(&cand_path)
            .with_context(|| format!("failed to remove {}", cand_path.display()))?;
    }
    Ok(rule)
}

pub(crate) fn load_candidate(repo_root: &Path, id: &str) -> Result<RatchetCandidate> {
    let path = candidates_dir(repo_root).join(format!("{id}.json"));
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("candidate `{id}` not found at {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("invalid JSON at {}", path.display()))
}

fn save_candidate(dir: &Path, candidate: &RatchetCandidate) -> Result<()> {
    let path = dir.join(format!("{}.json", candidate.id));
    let json = serde_json::to_string_pretty(candidate)?;
    atomic_write(&path, format!("{json}\n").as_bytes())
}

fn read_json_dir<T: for<'de> Deserialize<'de>>(dir: &Path) -> Result<Vec<T>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    paths.sort();
    let mut items = Vec::with_capacity(paths.len());
    for path in paths {
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let item: T = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        items.push(item);
    }
    Ok(items)
}
