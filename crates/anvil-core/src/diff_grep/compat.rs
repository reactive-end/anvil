//! Flat `rel:line: text` view of a search, kept for callers of the original `compressed_grep` API.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::search::SearchBackend;
use crate::diff_grep::grep::{GrepOptions, grep_with_backend};

const CHARS_PER_TOKEN: usize = 4;
const MIN_USEFUL_CHARS: usize = 16;

/// Token-bounded grep output across non-noise repository text files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompressedGrep {
    pub matches: Vec<String>,
    pub text: String,
    pub total_matches: usize,
    pub files_searched: usize,
    pub truncated: bool,
    pub estimated_tokens_saved: u64,
}

/// Searches `repo_root` for `pattern` with the built-in engine and flattens the result.
///
/// `total_matches` is exact even when `max_matches` or `max_tokens` cut the listing short.
pub fn compressed_grep(
    repo_root: &Path,
    pattern: &str,
    max_matches: usize,
    max_tokens: usize,
) -> Result<CompressedGrep> {
    let mut opts = GrepOptions::new(pattern);
    opts.max_matches = max_matches;
    opts.max_per_file = max_matches;
    opts.max_tokens = max_tokens;
    let report = grep_with_backend(repo_root, &opts, SearchBackend::Builtin)?;

    let budget = max_tokens.saturating_mul(CHARS_PER_TOKEN);
    let mut matches = Vec::new();
    let mut used = 0usize;
    let mut truncated = report.truncated;
    'files: for file in &report.files {
        for m in &file.matches {
            let entry = format!("{}:{}: {}", file.path, m.line, m.text);
            let sep = usize::from(!matches.is_empty());
            let remaining = budget.saturating_sub(used + sep);
            if remaining == 0 || (remaining < MIN_USEFUL_CHARS && !matches.is_empty()) {
                truncated = true;
                break 'files;
            }
            let entry = if entry.len() > remaining {
                truncated = true;
                truncate_utf8_ellipsis(&entry, remaining)
            } else {
                entry
            };
            used += sep + entry.len();
            matches.push(entry);
        }
    }

    Ok(CompressedGrep {
        text: matches.join("\n"),
        matches,
        total_matches: report.total_matches,
        files_searched: report.files_searched,
        truncated,
        estimated_tokens_saved: report.estimated_tokens_saved,
    })
}

fn truncate_utf8_ellipsis(s: &str, max_bytes: usize) -> String {
    let keep = if max_bytes <= 3 { max_bytes } else { max_bytes - 3 };
    let mut end = keep.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    if max_bytes <= 3 { s[..end].to_owned() } else { format!("{}...", &s[..end]) }
}
