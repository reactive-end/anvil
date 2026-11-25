//! Boundary-aware compiler/typechecker output filtering for `anvil_graph::impact`.

use std::path::Path;

use anyhow::Result;
use anvil_core::compressor::compress_output;
use anvil_core::traits::CompressedOutput;

struct PartitionResult<'a> {
    kept: Vec<&'a str>,
    suppressed_other_file: usize,
    unattributed_errors: usize,
}

/// Filters compiler/typechecker output to `impacted_files` without masking unattributed failures.
pub fn filter_typecheck_output(
    repo_root: Option<&Path>,
    label: &str,
    exit_code: i32,
    raw_output: &str,
    impacted_files: &[String],
    max_lines: usize,
    max_tokens: usize,
) -> Result<CompressedOutput> {
    if impacted_files.is_empty() {
        return compress_output(repo_root, label, exit_code, raw_output, max_lines, max_tokens);
    }

    let patterns = normalize_patterns(impacted_files);
    let part = partition_diagnostics(raw_output, &patterns);

    if exit_code == 0 {
        let filtered = part.kept.join("\n");
        return compress_output(repo_root, label, 0, &filtered, max_lines, max_tokens);
    }

    let timed_out = exit_code == 124 || raw_output.contains("[anvil] process timed out");
    let has_kept_error = part.kept.iter().any(|l| looks_like_error_line(l));
    if !timed_out
        && !has_kept_error
        && part.suppressed_other_file > 0
        && part.unattributed_errors == 0
    {
        let tool = label.split_whitespace().next().unwrap_or(label).to_owned();
        let count = part.suppressed_other_file;
        return Ok(CompressedOutput {
            tool: tool.clone(),
            passed: true,
            exit_code: 0,
            summary: format!(
                "PASS ({tool}): 0 errors in modified/impacted files ({count} unrelated error(s) suppressed)"
            ),
            lines: Vec::new(),
            total_errors: 0,
            truncated: false,
            raw_log_path: None,
        });
    }

    let payload = if has_kept_error {
        part.kept.join("\n")
    } else {
        raw_output.to_owned()
    };
    compress_output(repo_root, label, exit_code, &payload, max_lines, max_tokens)
}

fn normalize_patterns(impacted_files: &[String]) -> Vec<(String, String)> {
    impacted_files
        .iter()
        .map(|f| {
            let clean = f.trim_start_matches("./").replace('\\', "/");
            let win = clean.replace('/', "\\");
            (clean, win)
        })
        .collect()
}

fn partition_diagnostics<'a>(raw_output: &'a str, patterns: &[(String, String)]) -> PartitionResult<'a> {
    let mut kept = Vec::new();
    let mut suppressed_other_file = 0usize;
    let mut unattributed_errors = 0usize;
    let mut pending_header: Option<&'a str> = None;
    let mut in_kept_diag = false;

    for line in raw_output.lines() {
        let trimmed = line.trim();
        let is_indented = line.starts_with(char::is_whitespace);
        let matches_impacted = patterns
            .iter()
            .any(|(fwd, bck)| line_matches_path(line, fwd) || line_matches_path(line, bck));

        if matches_impacted {
            if let Some(hdr) = pending_header.take() {
                kept.push(hdr);
            }
            kept.push(line);
            in_kept_diag = true;
        } else if trimmed.starts_with("--> ") {
            if pending_header.take().is_some() {
                suppressed_other_file += 1;
            }
            in_kept_diag = false;
        } else if is_indented && in_kept_diag {
            kept.push(line);
        } else if looks_like_error_line(line) {
            in_kept_diag = false;
            if has_explicit_file_path(line) {
                if pending_header.take().is_some() {
                    unattributed_errors += 1;
                }
                suppressed_other_file += 1;
            } else if pending_header.replace(line).is_some() {
                unattributed_errors += 1;
            }
        } else if !is_indented || trimmed.is_empty() {
            in_kept_diag = false;
            if pending_header.take().is_some() {
                unattributed_errors += 1;
            }
        }
    }
    if pending_header.take().is_some() {
        unattributed_errors += 1;
    }

    PartitionResult {
        kept,
        suppressed_other_file,
        unattributed_errors,
    }
}

fn has_explicit_file_path(line: &str) -> bool {
    const EXTS: &[&str] = &[
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".py", ".kt", ".dart", ".php", ".gd",
        ".vue", ".svelte", ".go", ".java",
    ];
    let lower = line.to_ascii_lowercase();
    EXTS.iter().any(|ext| {
        lower.match_indices(ext).any(|(idx, _)| {
            let after = &lower[idx + ext.len()..];
            after.is_empty()
                || after.starts_with([':', '(', ' ', '\t', ',', ')', '\r', '\n'])
        })
    })
}

pub(crate) fn line_matches_path(line: &str, pat: &str) -> bool {
    if pat.is_empty() {
        return false;
    }
    for (idx, _) in line.match_indices(pat) {
        let prev_ok = line[..idx]
            .chars()
            .next_back()
            .is_none_or(|c| !is_path_ident_char(c));
        let after = &line[idx + pat.len()..];
        let next_ok = after
            .chars()
            .next()
            .is_none_or(|c| !is_path_ident_char(c) && c != '/' && c != '\\');
        if prev_ok && next_ok {
            return true;
        }
    }
    false
}

fn is_path_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}

fn looks_like_error_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && (trimmed.contains("error TS")
            || trimmed.contains(": error")
            || trimmed.contains(" - error")
            || trimmed.starts_with("error - ")
            || trimmed.starts_with("error[")
            || trimmed.starts_with("e: ")
            || trimmed.contains("[anvil] process timed out")
            || trimmed.contains("Parse error:")
            || trimmed.contains("Fatal error:")
            || has_php_file_line_diag(trimmed))
}

fn has_php_file_line_diag(trimmed: &str) -> bool {
    let Some(idx) = trimmed.find(".php:") else {
        return false;
    };
    let rest = &trimmed[idx + 5..];
    rest.chars().next().is_some_and(|c| c.is_ascii_digit())
}
