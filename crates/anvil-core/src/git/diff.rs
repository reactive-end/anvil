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
