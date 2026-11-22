//! Noise-filtered, binary-safe, token-bounded repository grep.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::diff_grep::is_noise_file;
use crate::fs_util::relativize_path;
use crate::stats::estimate_tokens;

const MAX_GREP_FILE_BYTES: u64 = 512 * 1024;
const IGNORED_DIRS: [&str; 6] = [".git", "target", "node_modules", "dist", "build", ".next"];

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

struct GrepState {
    matches: Vec<String>,
    used_chars: usize,
    char_budget: usize,
    max_matches: usize,
    total_matches: usize,
    files_searched: usize,
    raw_matched_bytes: usize,
    truncated: bool,
}

/// Searches `repo_root` for `pattern`, skipping ignored dirs, noise files, binaries, and files > 512 KB.
pub fn compressed_grep(
    repo_root: &Path,
    pattern: &str,
    max_matches: usize,
    max_tokens: usize,
) -> Result<CompressedGrep> {
    let regex = Regex::new(pattern).with_context(|| format!("invalid grep regex: {pattern}"))?;
    let mut state = GrepState {
        matches: Vec::new(),
        used_chars: 0,
        char_budget: max_tokens.saturating_mul(4),
        max_matches,
        total_matches: 0,
        files_searched: 0,
        raw_matched_bytes: 0,
        truncated: false,
    };

    if repo_root.is_dir() {
        walk_dir(repo_root, repo_root, &regex, &mut state)?;
    }

    let text = state.matches.join("\n");
    let estimated_tokens_saved =
        estimate_tokens(state.raw_matched_bytes).saturating_sub(estimate_tokens(text.len()));

    Ok(CompressedGrep {
        matches: state.matches,
        text,
        total_matches: state.total_matches,
        files_searched: state.files_searched,
        truncated: state.truncated,
        estimated_tokens_saved,
    })
}

fn walk_dir(root: &Path, dir: &Path, regex: &Regex, state: &mut GrepState) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };

    let mut children = Vec::new();
    for entry in entries.flatten() {
        children.push(entry.path());
    }
    children.sort();

    for path in children {
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let rel = relativize_path(root, &path);
        if meta.is_dir() {
            if should_skip_dir(&path, &rel) {
                continue;
            }
            walk_dir(root, &path, regex, state)?;
        } else if meta.is_file() && meta.len() <= MAX_GREP_FILE_BYTES && !is_noise_file(&rel) {
            search_file(&path, &rel, regex, state);
        }
    }
    Ok(())
}

fn should_skip_dir(path: &Path, rel: &str) -> bool {
    if rel == ".harness/.state" || rel.starts_with(".harness/.state/") {
        return true;
    }
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| IGNORED_DIRS.contains(&name))
}

fn search_file(path: &Path, rel: &str, regex: &Regex, state: &mut GrepState) {
    let Ok(mut file) = File::open(path) else {
        return;
    };
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return;
    }
    if buf.contains(&0) {
        return;
    }
    let Ok(content) = std::str::from_utf8(&buf) else {
        return;
    };

    state.files_searched = state.files_searched.saturating_add(1);
    for (idx, line) in content.lines().enumerate() {
        if !regex.is_match(line) {
            continue;
        }
        state.total_matches = state.total_matches.saturating_add(1);
        state.raw_matched_bytes = state.raw_matched_bytes.saturating_add(line.len() + 1);
        let formatted = format!("{rel}:{}: {}", idx + 1, line.trim());
        push_match(state, formatted);
    }
}

fn push_match(state: &mut GrepState, formatted: String) {
    if state.truncated {
        return;
    }
    if state.matches.len() >= state.max_matches || state.char_budget == 0 {
        state.truncated = true;
        return;
    }
    let sep = usize::from(!state.matches.is_empty());
    let needed = sep + formatted.len();
    if state.used_chars.saturating_add(needed) > state.char_budget {
        state.truncated = true;
        return;
    }
    state.used_chars += needed;
    state.matches.push(formatted);
}
