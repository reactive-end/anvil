//! Noise-filtered, line/token-capped unified diff compressor.

use serde::{Deserialize, Serialize};

use crate::diff_grep::is_noise_file;
use crate::stats::estimate_tokens;

/// Token-bounded compressed git diff output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompressedDiff {
    pub text: String,
    pub lines: Vec<String>,
    pub skipped_files: Vec<String>,
    pub truncated: bool,
    pub raw_bytes: usize,
    pub compact_bytes: usize,
    pub estimated_tokens_saved: u64,
}

/// Drops noisy file hunks, strips redundant metadata (`index ...`, `similarity index`),
/// summarizes skipped files in one line, and enforces `max_lines` and `max_tokens`.
pub fn compress_diff(raw_diff: &str, max_lines: usize, max_tokens: usize) -> CompressedDiff {
    let raw_bytes = raw_diff.len();
    if raw_diff.trim().is_empty() {
        return CompressedDiff {
            text: String::new(),
            lines: Vec::new(),
            skipped_files: Vec::new(),
            truncated: false,
            raw_bytes,
            compact_bytes: 0,
            estimated_tokens_saved: 0,
        };
    }

    let (kept_lines, skipped_files) = filter_diff_sections(raw_diff);
    let mut candidate_lines = Vec::with_capacity(kept_lines.len() + 1);
    if !skipped_files.is_empty() {
        candidate_lines.push(format_skipped_summary(&skipped_files));
    }
    candidate_lines.extend(kept_lines);

    let (lines, truncated) = cap_diff_lines(&candidate_lines, max_lines, max_tokens);
    let text = lines.join("\n");
    let compact_bytes = text.len();
    let estimated_tokens_saved =
        estimate_tokens(raw_bytes).saturating_sub(estimate_tokens(compact_bytes));

    CompressedDiff {
        text,
        lines,
        skipped_files,
        truncated,
        raw_bytes,
        compact_bytes,
        estimated_tokens_saved,
    }
}

fn filter_diff_sections(raw_diff: &str) -> (Vec<String>, Vec<String>) {
    let mut kept = Vec::new();
    let mut skipped = Vec::new();
    let mut skipping_current = false;

    for line in raw_diff.lines() {
        if line.starts_with("diff --git ") {
            if let Some(rel_path) = extract_diff_path(line)
                && is_noise_file(&rel_path)
            {
                skipping_current = true;
                skipped.push(rel_path);
                continue;
            }
            skipping_current = false;
            kept.push(line.to_owned());
            continue;
        }

        if skipping_current || is_redundant_meta(line) {
            continue;
        }
        kept.push(line.to_owned());
    }

    (kept, skipped)
}

fn is_redundant_meta(line: &str) -> bool {
    line.starts_with("index ")
        || line.starts_with("similarity index ")
        || line.starts_with("dissimilarity index ")
}

fn extract_diff_path(header_line: &str) -> Option<String> {
    let rest = header_line.strip_prefix("diff --git ")?.trim();
    if let Some(idx) = rest.rfind("\" \"b/") {
        let b_part = &rest[idx + 3..];
        return Some(b_part.trim_end_matches('"').to_owned());
    }
    if let Some((_, b_part)) = rest.rsplit_once(" b/") {
        return Some(b_part.trim_matches('"').to_owned());
    }
    None
}

fn format_skipped_summary(skipped: &[String]) -> String {
    const MAX_LISTED: usize = 8;
    if skipped.len() <= MAX_LISTED {
        format!("skipped {} noisy file(s): {}", skipped.len(), skipped.join(", "))
    } else {
        let head = skipped[..MAX_LISTED].join(", ");
        let extra = skipped.len() - MAX_LISTED;
        format!("skipped {} noisy file(s): {head}, +{extra} more", skipped.len())
    }
}

fn cap_diff_lines(lines: &[String], max_lines: usize, max_tokens: usize) -> (Vec<String>, bool) {
    if lines.is_empty() {
        return (Vec::new(), false);
    }
    if max_lines == 0 || max_tokens == 0 {
        return (Vec::new(), true);
    }

    let char_budget = max_tokens.saturating_mul(4);
    let mut out = Vec::with_capacity(lines.len().min(max_lines));
    let mut used_chars = 0usize;
    let mut truncated = false;

    for line in lines {
        if out.len() >= max_lines {
            truncated = true;
            break;
        }
        let sep = usize::from(!out.is_empty());
        let needed = sep + line.len();
        if used_chars.saturating_add(needed) > char_budget {
            truncated = true;
            break;
        }
        used_chars += needed;
        out.push(line.clone());
    }

    (out, truncated)
}
