//! Tier 0 check: windows of added lines that already exist elsewhere in the repository.

mod lines;

use std::fs;
use std::path::Path;
use std::thread;

use rustc_hash::FxHashMap;

use crate::git::AddedLines;
use crate::sonar::has_allow_comment;
use crate::traits::{Diagnostic, Severity};
use lines::{
    collapse, hashed_lines, in_ranges, is_checked_path, is_substantial, language_key, normalize,
    window_hash,
};

/// Rule id of every finding.
pub const RULE: &str = "duplication:new-code";
const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;
const ALLOW_RULES: [&str; 2] = ["duplication", "sonar:duplication"];

/// A window made only of added lines.
struct Block {
    file: String,
    idx: usize,
    line: u32,
    text: Vec<String>,
}

/// An added block and the place it already exists.
struct Hit {
    file: String,
    idx: usize,
    line: u32,
    other_file: String,
    other_idx: usize,
    other_line: u32,
}

/// Compares the added lines in `added` against every file in `repo_files` (repository-relative paths).
pub fn check_added_duplication(
    root: &Path,
    repo_files: &[String],
    added: &AddedLines,
    window: usize,
) -> Vec<Diagnostic> {
    let window = window.max(4);
    let blocks = collect_blocks(root, added, window);
    if blocks.is_empty() {
        return Vec::new();
    }
    let mut hits = scan_repository(root, repo_files, added, &blocks, window);
    hits.sort_by(|a, b| (&a.file, a.idx, &a.other_file).cmp(&(&b.file, b.idx, &b.other_file)));
    report(&hits, window)
}

fn read_source(root: &Path, rel: &str) -> Option<String> {
    let path = root.join(rel);
    let small = fs::metadata(&path).ok()?.len() <= MAX_SOURCE_BYTES;
    small.then(|| fs::read_to_string(path).ok()).flatten()
}

fn all_added<T>(ranges: &[(u32, u32)], window: &[(usize, T)]) -> bool {
    window.iter().all(|(idx, _)| in_ranges(ranges, u32::try_from(idx + 1).unwrap_or(u32::MAX)))
}

fn collect_blocks(root: &Path, added: &AddedLines, window: usize) -> FxHashMap<u64, Vec<Block>> {
    let mut blocks: FxHashMap<u64, Vec<Block>> = FxHashMap::default();
    let mut files: Vec<&String> = added.keys().filter(|f| is_checked_path(f)).collect();
    files.sort();
    for file in files {
        let Some(source) = read_source(root, file) else {
            continue;
        };
        let raw: Vec<&str> = source.lines().collect();
        let norm = normalize(&raw);
        let hashed = hashed_lines(&raw);
        for (start, win) in norm.windows(window).enumerate() {
            let first = win[0].0;
            let allowed = ALLOW_RULES.iter().any(|rule| has_allow_comment(&raw, first, rule));
            if allowed || !all_added(&added[file], win) || !is_substantial(win) {
                continue;
            }
            let block = Block {
                file: file.clone(),
                idx: start,
                line: u32::try_from(first + 1).unwrap_or(1),
                text: win.iter().map(|(_, l)| l.clone()).collect(),
            };
            blocks
                .entry(window_hash(language_key(file), &hashed[start..start + window]))
                .or_default()
                .push(block);
        }
    }
    blocks
}

/// Reading the tracked files dominates the cost, so the files are split across threads.
fn scan_repository(
    root: &Path,
    repo_files: &[String],
    added: &AddedLines,
    blocks: &FxHashMap<u64, Vec<Block>>,
    window: usize,
) -> Vec<Hit> {
    let files: Vec<&String> = repo_files.iter().filter(|f| is_checked_path(f)).collect();
    let threads = thread::available_parallelism().map_or(1, usize::from).min(8);
    let chunk = files.len().div_ceil(threads).max(1);
    thread::scope(|scope| {
        let workers: Vec<_> = files
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .flat_map(|rel| scan_file(root, rel, added, blocks, window))
                        .collect::<Vec<Hit>>()
                })
            })
            .collect();
        workers.into_iter().flat_map(|worker| worker.join().unwrap_or_default()).collect()
    })
}

fn scan_file(
    root: &Path,
    rel: &str,
    added: &AddedLines,
    blocks: &FxHashMap<u64, Vec<Block>>,
    window: usize,
) -> Vec<Hit> {
    let mut hits = Vec::new();
    let Some(source) = read_source(root, rel) else {
        return hits;
    };
    let raw: Vec<&str> = source.lines().collect();
    let hashed = hashed_lines(&raw);
    for (start, win) in hashed.windows(window).enumerate() {
        let Some(candidates) = blocks.get(&window_hash(language_key(rel), win)) else {
            continue;
        };
        let text: Vec<String> = win.iter().map(|&(idx, _)| collapse(raw[idx])).collect();
        let other_is_new = added.get(rel).is_some_and(|ranges| all_added(ranges, win));
        for block in candidates.iter().filter(|b| b.text == text) {
            let same_place = block.file == rel && block.idx.abs_diff(start) < window;
            let reported_by_other = other_is_new && (block.file.as_str(), block.idx) < (rel, start);
            if !same_place && !reported_by_other {
                hits.push(Hit {
                    file: block.file.clone(),
                    idx: block.idx,
                    line: block.line,
                    other_file: rel.to_owned(),
                    other_idx: start,
                    other_line: u32::try_from(win[0].0 + 1).unwrap_or(1),
                });
            }
        }
    }
    hits
}

/// One diagnostic per duplicated block: a run of overlapping windows is a single block.
fn report(hits: &[Hit], window: usize) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut covered: (&str, usize) = ("", 0);
    let mut previous: Option<&Hit> = None;
    for hit in hits {
        let continues = previous.is_some_and(|p| {
            p.file == hit.file
                && p.other_file == hit.other_file
                && p.idx + 1 == hit.idx
                && p.other_idx + 1 == hit.other_idx
        });
        if continues || (covered.0 == hit.file && hit.idx < covered.1) {
            covered = (hit.file.as_str(), hit.idx + window);
            previous = Some(hit);
            continue;
        }
        covered = (hit.file.as_str(), hit.idx + window);
        previous = Some(hit);
        diagnostics.push(Diagnostic {
            rule: RULE.to_owned(),
            severity: Severity::Error,
            file: hit.file.clone(),
            line: hit.line,
            column: 1,
            message: format!("{window}+ new lines duplicate `{}:{}`", hit.other_file, hit.other_line),
            suggestion: Some(
                "reuse the existing code, extract a shared helper, or add anvil-allow(duplication): <reason>"
                    .to_owned(),
            ),
        });
    }
    diagnostics
}
