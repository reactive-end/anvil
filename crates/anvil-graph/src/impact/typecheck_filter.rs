//! Boundary-aware compiler/typechecker output filtering for `anvil_graph::impact`.

use std::path::Path;

use anyhow::Result;
use anvil_core::compressor::compress_output;
use anvil_core::traits::CompressedOutput;

/// Filters compiler/typechecker output to `impacted_files` without leaking unrelated errors.
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

    let patterns: Vec<(String, String)> = impacted_files
        .iter()
        .map(|f| {
            let clean = f.trim_start_matches("./").replace('\\', "/");
            let win = clean.replace('/', "\\");
            (clean, win)
        })
        .collect();

    let mut kept = Vec::new();
    let mut suppressed_errors = 0usize;
    let mut in_kept_diag = false;

    for line in raw_output.lines() {
        let is_indented = line.starts_with(char::is_whitespace);
        let matches_impacted = patterns
            .iter()
            .any(|(fwd, bck)| line_matches_path(line, fwd) || line_matches_path(line, bck));

        if matches_impacted {
            kept.push(line);
            in_kept_diag = true;
        } else if is_indented && in_kept_diag {
            kept.push(line);
        } else {
            in_kept_diag = false;
            if looks_like_error_line(line) {
                suppressed_errors += 1;
            }
        }
    }

    if exit_code == 0 {
        let filtered = kept.join("\n");
        return compress_output(repo_root, label, 0, &filtered, max_lines, max_tokens);
    }

    let has_kept_error = kept.iter().any(|l| looks_like_error_line(l));
    if kept.is_empty() || (suppressed_errors > 0 && !has_kept_error) {
        let tool = label.split_whitespace().next().unwrap_or(label).to_owned();
        let count = suppressed_errors.max(1);
        return Ok(CompressedOutput {
            tool,
            passed: true,
            exit_code: 0,
            summary: format!(
                "PASS ({}): 0 errors in modified/impacted files ({count} unrelated error(s) suppressed)",
                label.split_whitespace().next().unwrap_or(label)
            ),
            lines: Vec::new(),
            total_errors: 0,
            truncated: false,
            raw_log_path: None,
        });
    }

    let filtered = kept.join("\n");
    compress_output(repo_root, label, exit_code, &filtered, max_lines, max_tokens)
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
