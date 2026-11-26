//! Batched unified diff retrieval and per-file diff chunk splitting.

use rustc_hash::FxHashMap;
use std::path::Path;

use anyhow::Result;

use super::{head_commit, run_git_unquoted};

/// Runs ONE `git diff -M HEAD` call and splits output by `diff --git a/... b/...` into a map by path.
pub fn batched_working_tree_diffs(repo_root: &Path) -> Result<FxHashMap<String, String>> {
    let raw = match run_git_unquoted(repo_root, &["diff", "-M", "HEAD"]) {
        Ok(out) => out,
        Err(err) if head_commit(repo_root).is_err() => {
            run_git_unquoted(repo_root, &["diff", "--cached", "-M"]).map_err(|_| err)?
        }
        Err(err) => return Err(err),
    };
    Ok(split_batched_diffs(&raw))
}

/// Runs ONE `git diff --cached -M --unified=1` call and splits output by destination path.
pub fn batched_staged_diffs(repo_root: &Path) -> Result<FxHashMap<String, String>> {
    let raw = run_git_unquoted(repo_root, &["diff", "--cached", "-M", "--unified=1"])?;
    Ok(split_batched_diffs(&raw))
}

fn split_batched_diffs(raw: &str) -> FxHashMap<String, String> {
    let mut diffs = FxHashMap::default();
    let mut current_header = String::new();
    let mut current_chunk = String::new();

    for line in raw.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            flush_diff_chunk(&current_header, &current_chunk, &mut diffs);
            current_header = line.trim_end_matches(['\r', '\n']).to_owned();
            current_chunk.clear();
        }
        current_chunk.push_str(line);
    }
    flush_diff_chunk(&current_header, &current_chunk, &mut diffs);
    diffs
}

fn flush_diff_chunk(header: &str, chunk: &str, out: &mut FxHashMap<String, String>) {
    if header.is_empty() || chunk.is_empty() {
        return;
    }
    if let Some(path) = extract_diff_path(header, chunk) {
        out.insert(path, chunk.to_owned());
    }
}

fn extract_diff_path(header: &str, chunk: &str) -> Option<String> {
    let mut minus_path: Option<String> = None;
    for raw_line in chunk.lines() {
        let line = raw_line.trim_end_matches('\r');
        if let Some(p) = line.strip_prefix("rename to ") {
            return Some(p.trim_end_matches('\t').replace('\\', "/"));
        }
        if let Some(p) = line.strip_prefix("+++ b/") {
            return Some(p.trim_end_matches('\t').replace('\\', "/"));
        }
        if let Some(p) = line.strip_prefix("--- a/") {
            minus_path = Some(p.trim_end_matches('\t').replace('\\', "/"));
        }
        if line == "+++ /dev/null"
            && let Some(deleted) = minus_path.take()
        {
            return Some(deleted);
        }
    }

    let rest = header.strip_prefix("diff --git a/")?;
    if rest.len() > 3 && (rest.len() - 3).wrapping_rem(2) == 0 {
        let mid = (rest.len() - 3) / 2;
        if rest.is_char_boundary(mid)
            && &rest[mid..mid + 3] == " b/"
            && rest[..mid] == rest[mid + 3..]
        {
            return Some(rest[..mid].replace('\\', "/"));
        }
    }
    rest.rsplit_once(" b/")
        .map(|(_, b)| b.replace('\\', "/"))
}

/// Inclusive 1-based line ranges added to each file, keyed by repository-relative path.
pub type AddedLines = FxHashMap<String, Vec<(u32, u32)>>;

/// Added line ranges of every file in a raw multi-file unified diff (`git diff -M ...` output).
pub fn parse_added_lines(raw_diff: &str) -> AddedLines {
    added_lines_of(&split_batched_diffs(raw_diff))
}

/// Added line ranges of each already-split per-file diff.
pub fn added_lines_of(diffs: &FxHashMap<String, String>) -> AddedLines {
    diffs
        .iter()
        .filter_map(|(path, diff)| {
            let ranges = hunk_ranges(diff);
            (!ranges.is_empty()).then(|| (path.clone(), ranges))
        })
        .collect()
}

fn hunk_ranges(diff: &str) -> Vec<(u32, u32)> {
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    let mut next_line = 0u32;
    let mut in_hunk = false;
    for line in diff.lines() {
        if let Some(start) = line.strip_prefix("@@ ").and_then(new_start) {
            next_line = start;
            in_hunk = true;
        } else if !in_hunk {
            continue;
        } else if line.starts_with('+') {
            match ranges.last_mut() {
                Some((_, end)) if *end + 1 == next_line => *end = next_line,
                _ => ranges.push((next_line, next_line)),
            }
            next_line += 1;
        } else if line.starts_with(' ') || line.is_empty() {
            next_line += 1;
        }
    }
    ranges
}

/// The `c` of a hunk header `-a,b +c,d @@`.
fn new_start(header: &str) -> Option<u32> {
    let plus = header.split_whitespace().find(|part| part.starts_with('+'))?;
    plus.trim_start_matches('+').split(',').next()?.parse().ok()
}
