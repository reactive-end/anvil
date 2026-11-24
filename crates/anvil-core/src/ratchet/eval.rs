//! Promoted ratchet rule evaluation and candidate regex benchmarking across repository files.

use std::fs;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::fs_util::relativize_path;
use crate::traits::{Diagnostic, Severity};

use super::matcher::{
    collect_repo_files, is_match_in_comment_or_string, is_suppressed, matches_target,
};
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

/// Pre-compiled promoted ratchet rules for O(1) regex reuse across multiple files.
#[derive(Debug, Clone, Default)]
pub struct CompiledPromotedRules {
    rules: Vec<CompiledRule>,
}

#[derive(Debug, Clone)]
struct CompiledRule {
    id: String,
    rule_name: String,
    description: String,
    file_globs: Vec<String>,
    languages: Vec<String>,
    severity: Severity,
    multiline: bool,
    match_comments_and_strings: bool,
    regex: Regex,
}

impl CompiledPromotedRules {
    /// Returns the number of valid compiled rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Returns true when no compiled rules are active.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// Loads all promoted rules from `.harness/rules/*.json`.
pub fn load_promoted_rules(repo_root: &Path) -> Result<Vec<PromotedRule>> {
    list_rules(repo_root)
}

/// Compiles promoted rule regexes once so callers can evaluate many files without recompiling.
pub fn compile_promoted_rules(rules: &[PromotedRule]) -> CompiledPromotedRules {
    let mut compiled = Vec::with_capacity(rules.len());
    for rule in rules {
        let Some(pat) = rule.pattern.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let Ok(regex) = Regex::new(pat) else {
            continue;
        };
        compiled.push(CompiledRule {
            id: rule.id.clone(),
            rule_name: rule.rule_name.clone(),
            description: rule.description.clone(),
            file_globs: rule.file_globs.clone(),
            languages: rule.languages.clone(),
            severity: rule.severity.unwrap_or(Severity::Warning),
            multiline: rule.multiline || pat.starts_with("(?s)") || pat.contains("\\n"),
            match_comments_and_strings: rule.match_comments_and_strings,
            regex,
        });
    }
    CompiledPromotedRules { rules: compiled }
}

/// Evaluates pre-compiled promoted rules against a single file's `source`,
/// honoring `anvil-allow(<id>):` only in comment contexts (`//`, `/*`, `#`, `--`).
pub fn evaluate_compiled(
    compiled: &CompiledPromotedRules,
    rel_path: &str,
    source: &str,
) -> Vec<Diagnostic> {
    let norm_path = rel_path.replace('\\', "/");
    let matching: Vec<&CompiledRule> = compiled
        .rules
        .iter()
        .filter(|r| matches_target(&norm_path, &r.file_globs, &r.languages))
        .collect();
    if matching.is_empty() {
        return Vec::new();
    }

    let lines: Vec<&str> = source.lines().collect();
    let mut diagnostics = Vec::new();
    for rule in matching {
        if rule.multiline {
            eval_multiline_rule(rule, &norm_path, source, &lines, &mut diagnostics);
        } else {
            eval_single_line_rule(rule, &norm_path, &lines, &mut diagnostics);
        }
    }
    diagnostics
}

fn eval_multiline_rule(
    rule: &CompiledRule,
    norm_path: &str,
    source: &str,
    lines: &[&str],
    diagnostics: &mut Vec<Diagnostic>,
) {
    for m in rule.regex.find_iter(source) {
        let prefix = &source[..m.start()];
        let line_idx = prefix.bytes().filter(|&b| b == b'\n').count();
        let line_start = prefix.rfind('\n').map_or(0, |p| p + 1);
        let col = (m.start() - line_start + 1) as u32;
        if line_idx < lines.len() && is_suppressed(lines, line_idx, &rule.id, &rule.rule_name) {
            continue;
        }
        diagnostics.push(build_diagnostic(
            rule,
            norm_path,
            (line_idx + 1) as u32,
            col,
        ));
    }
}

fn eval_single_line_rule(
    rule: &CompiledRule,
    norm_path: &str,
    lines: &[&str],
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (idx, line) in lines.iter().enumerate() {
        if is_suppressed(lines, idx, &rule.id, &rule.rule_name) {
            continue;
        }
        for m in rule.regex.find_iter(line) {
            if !rule.match_comments_and_strings && is_match_in_comment_or_string(line, m.start()) {
                continue;
            }
            diagnostics.push(build_diagnostic(
                rule,
                norm_path,
                (idx + 1) as u32,
                (m.start() + 1) as u32,
            ));
        }
    }
}

fn build_diagnostic(rule: &CompiledRule, norm_path: &str, line: u32, column: u32) -> Diagnostic {
    Diagnostic {
        rule: rule.rule_name.clone(),
        severity: rule.severity,
        file: norm_path.to_owned(),
        line,
        column,
        message: rule.description.clone(),
        suggestion: Some(format!(
            "Remove or suppress with `anvil-allow({}): <reason>`",
            rule.id
        )),
    }
}

/// Evaluates `rules` against a single file's `source`, honoring `anvil-allow(<rule_id>):`.
pub fn evaluate_promoted_rules(
    rules: &[PromotedRule],
    rel_path: &str,
    source: &str,
) -> Vec<Diagnostic> {
    let compiled = compile_promoted_rules(rules);
    evaluate_compiled(&compiled, rel_path, source)
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
