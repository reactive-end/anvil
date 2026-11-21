//! Promoted ratchet rule evaluation and candidate regex benchmarking across repository files.

use std::fs;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::fs_util::relativize_path;
use crate::traits::{Diagnostic, Severity};

use super::matcher::matches_target;
use super::{MAX_RUNTIME_MS, PromotedRule, list_rules, load_candidate};

const MAX_SCAN_FILE_BYTES: u64 = 512 * 1024;
const MAX_SAMPLE_MATCHES: usize = 20;

/// Single regex match captured during [`test_candidate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateTestMatch {
    pub file: String,
    pub line: u32,
    pub snippet: String,
}

/// Performance and hit summary from testing a candidate regex across repository files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateTestReport {
    pub id: String,
    pub rule_name: String,
    pub files_scanned: usize,
    pub match_count: usize,
    pub elapsed_ms: u64,
    pub under_budget: bool,
    pub sample_matches: Vec<CandidateTestMatch>,
}

/// Loads all promoted rules from `.harness/rules/*.json`.
pub fn load_promoted_rules(repo_root: &Path) -> Result<Vec<PromotedRule>> {
    list_rules(repo_root)
}

/// Evaluates `rules` against a single file's `source`, honoring `anvil-allow(<rule_id>):`.
pub fn evaluate_promoted_rules(
    rules: &[PromotedRule],
    rel_path: &str,
    source: &str,
) -> Vec<Diagnostic> {
    let norm_path = rel_path.replace('\\', "/");
    let lines: Vec<&str> = source.lines().collect();
    let mut diagnostics = Vec::new();

    for rule in rules {
        let Some(pat) = rule.pattern.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        if !matches_target(&norm_path, &rule.file_globs, &rule.languages) {
            continue;
        }
        let Ok(re) = Regex::new(pat) else {
            continue;
        };
        let severity = rule.severity.unwrap_or(Severity::Warning);

        for (idx, line) in lines.iter().enumerate() {
            let Some(m) = re.find(line) else {
                continue;
            };
            if is_suppressed(&lines, idx, &rule.id, &rule.rule_name) {
                continue;
            }
            diagnostics.push(Diagnostic {
                rule: rule.rule_name.clone(),
                severity,
                file: norm_path.clone(),
                line: (idx + 1) as u32,
                column: (m.start() + 1) as u32,
                message: rule.description.clone(),
                suggestion: Some(format!(
                    "Remove or suppress with `anvil-allow({}): <reason>`",
                    rule.id
                )),
            });
        }
    }
    diagnostics
}

/// Runs a candidate's regex across matching repository files and measures match count and runtime.
pub fn test_candidate(repo_root: &Path, candidate_id: &str) -> Result<CandidateTestReport> {
    let candidate = load_candidate(repo_root, candidate_id)?;
    let pat = candidate
        .pattern
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .with_context(|| format!("candidate `{candidate_id}` has no regex pattern"))?;
    let re = Regex::new(pat)
        .with_context(|| format!("invalid candidate regex pattern for `{candidate_id}`"))?;

    let start = Instant::now();
    let mut files = Vec::new();
    collect_repo_files(repo_root, &mut files)?;

    let mut files_scanned = 0usize;
    let mut match_count = 0usize;
    let mut sample_matches = Vec::new();

    for path in files {
        let rel = relativize_path(repo_root, &path);
        if !matches_target(&rel, &candidate.file_globs, &candidate.languages) {
            continue;
        }
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if meta.len() > MAX_SCAN_FILE_BYTES {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        files_scanned += 1;
        scan_file_matches(&re, &rel, &content, &mut match_count, &mut sample_matches);
    }

    let elapsed_ms = start.elapsed().as_millis() as u64;
    Ok(CandidateTestReport {
        id: candidate.id,
        rule_name: candidate.rule_name,
        files_scanned,
        match_count,
        elapsed_ms,
        under_budget: elapsed_ms < MAX_RUNTIME_MS,
        sample_matches,
    })
}

fn scan_file_matches(
    re: &Regex,
    rel: &str,
    content: &str,
    match_count: &mut usize,
    samples: &mut Vec<CandidateTestMatch>,
) {
    for (idx, line) in content.lines().enumerate() {
        let hits = re.find_iter(line).count();
        if hits == 0 {
            continue;
        }
        *match_count += hits;
        if samples.len() < MAX_SAMPLE_MATCHES {
            samples.push(CandidateTestMatch {
                file: rel.to_owned(),
                line: (idx + 1) as u32,
                snippet: line.trim().chars().take(120).collect(),
            });
        }
    }
}

fn is_suppressed(lines: &[&str], idx: usize, rule_id: &str, rule_name: &str) -> bool {
    let check_line = |line: &str| -> bool {
        let id_marker = format!("anvil-allow({rule_id}):");
        let name_marker = format!("anvil-allow({rule_name}):");
        line.contains(&id_marker) || line.contains(&name_marker)
    };
    check_line(lines[idx]) || (idx > 0 && check_line(lines[idx - 1]))
}

fn collect_repo_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        bail!("failed to read directory {}", dir.display());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if path.is_dir() {
            if is_ignored_dir(&name_str) {
                continue;
            }
            collect_repo_files(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn is_ignored_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".harness"
            | "target"
            | "node_modules"
            | "dist"
            | "build"
            | ".next"
            | ".gradle"
            | "__pycache__"
            | ".venv"
            | "venv"
    )
}
