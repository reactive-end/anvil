//! Gitignore-aware, symbol-annotatable, token-bounded repository search.
//!
//! Entry points are [`grep`] (backend from `.harness/config.json`) and [`grep_with_backend`]. Results
//! come back as a [`GrepReport`] that [`render`](crate::diff_grep::render) turns into compact text.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::AnvilConfig;
use crate::config::search::SearchBackend;
use crate::diff_grep::render::render;
use crate::diff_grep::{builtin, tgrep};
use crate::stats::estimate_tokens;

/// Characters of rendered output the engine reserves per requested token.
const CHARS_PER_TOKEN: usize = 4;
/// Fixed rendering overhead per shown row (line number, indentation, separators).
const ROW_OVERHEAD: usize = 8;

/// Search request. Build with [`GrepOptions::new`] and adjust the public fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrepOptions {
    /// Regex (or literal when `fixed_strings`) to search for.
    pub pattern: String,
    /// Repo-relative files or directories to search; empty searches the whole repository.
    pub paths: Vec<String>,
    /// Ripgrep-style glob filters (`*.rs`, `!tests/**`); a positive glob whitelists matching files.
    pub globs: Vec<String>,
    /// Always ignore case.
    pub ignore_case: bool,
    /// Ignore case unless the pattern has an uppercase letter (default on, ignored with `ignore_case`).
    pub smart_case: bool,
    /// Treat `pattern` as a literal string.
    pub fixed_strings: bool,
    /// Match whole words only.
    pub word: bool,
    /// Also search dot-files and dot-directories (`.github`, `.harness`, `.claude`, `.agents` are always searched).
    pub hidden: bool,
    /// Lines of context before and after every shown match.
    pub context: usize,
    /// Maximum matches shown across all files.
    pub max_matches: usize,
    /// Maximum matches shown per file.
    pub max_per_file: usize,
    /// Approximate token budget of the shown output.
    pub max_tokens: usize,
    /// List matching files only (with per-file match counts).
    pub files_only: bool,
}

impl GrepOptions {
    /// Creates options for `pattern` with the documented defaults (50 matches, 10 per file, 1500 tokens, smart case).
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            paths: Vec::new(),
            globs: Vec::new(),
            ignore_case: false,
            smart_case: true,
            fixed_strings: false,
            word: false,
            hidden: false,
            context: 0,
            max_matches: 50,
            max_per_file: 10,
            max_tokens: 1500,
            files_only: false,
        }
    }
}

/// One matching line plus its context and (when a code graph is available) enclosing symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineMatch {
    pub line: u32,
    pub text: String,
    pub before: Vec<(u32, String)>,
    pub after: Vec<(u32, String)>,
    /// Enclosing symbol such as `fn find_user`; filled by the graph-aware callers.
    pub symbol: Option<String>,
}

/// Shown matches of one file; `total` counts every match in the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMatches {
    pub path: String,
    pub matches: Vec<LineMatch>,
    pub total: usize,
}

/// Outcome of a search: shown matches plus exact totals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrepReport {
    pub files: Vec<FileMatches>,
    /// Exact number of matching lines, even when the shown output is truncated.
    pub total_matches: usize,
    pub files_with_matches: usize,
    pub files_searched: usize,
    pub truncated: bool,
    /// Engine that ran: `builtin` or `tgrep`.
    pub backend: String,
    /// Why a requested backend was not used, when applicable.
    pub note: Option<String>,
    pub estimated_tokens_saved: u64,
}

/// Per-file search result shared by every backend before caps are applied.
pub(crate) struct FileHits {
    pub path: String,
    pub total: usize,
    /// At most `max_per_file` matches, in line order.
    pub matches: Vec<LineMatch>,
    pub raw_bytes: usize,
}

/// Searches `root` using the backend configured in `.harness/config.json` (default `auto`).
pub fn grep(root: &Path, opts: &GrepOptions) -> Result<GrepReport> {
    let backend = AnvilConfig::load_or_default(root).map(|c| c.search.backend).unwrap_or_default();
    grep_with_backend(root, opts, backend)
}

/// Searches `root` with an explicit `backend`; `tgrep` problems fall back to the built-in engine with a `note`.
pub fn grep_with_backend(
    root: &Path,
    opts: &GrepOptions,
    backend: SearchBackend,
) -> Result<GrepReport> {
    grep_with_binary(root, opts, backend, tgrep::find_binary().as_deref())
}

/// Like [`grep_with_backend`] with the `tgrep` executable supplied by the caller (`None` = not installed).
pub fn grep_with_binary(
    root: &Path,
    opts: &GrepOptions,
    backend: SearchBackend,
    tgrep_binary: Option<&Path>,
) -> Result<GrepReport> {
    let use_tgrep = match backend {
        SearchBackend::Builtin => false,
        SearchBackend::Tgrep => true,
        SearchBackend::Auto => tgrep::auto_eligible(root, tgrep_binary),
    };
    if !use_tgrep {
        return builtin::run(root, opts);
    }
    match tgrep::run(root, opts, tgrep_binary) {
        Ok(report) => Ok(report),
        Err(err) => {
            let mut report = builtin::run(root, opts)?;
            report.note = Some(format!("tgrep unavailable ({err:#}); used the builtin engine"));
            Ok(report)
        }
    }
}

/// Running totals while per-file hits are folded into a report.
#[derive(Default)]
struct Tally {
    used: usize,
    shown: usize,
    raw_bytes: usize,
    total_matches: usize,
    files_with_matches: usize,
}

/// Applies the per-file, total-match and token caps to sorted per-file hits and builds the report.
pub(crate) fn assemble(
    opts: &GrepOptions,
    hits: Vec<FileHits>,
    files_searched: usize,
    backend: &str,
) -> GrepReport {
    let budget = opts.max_tokens.saturating_mul(CHARS_PER_TOKEN);
    let mut tally = Tally::default();
    let mut files = Vec::new();

    for hit in hits.into_iter().filter(|h| h.total > 0) {
        tally.total_matches += hit.total;
        tally.files_with_matches += 1;
        tally.raw_bytes += hit.raw_bytes;
        let kept = if opts.files_only {
            take_listing(opts, budget, &mut tally, &hit.path)
        } else {
            take_matches(opts, budget, &mut tally, &hit)
        };
        if let Some(matches) = kept {
            files.push(FileMatches { path: hit.path, matches, total: hit.total });
        }
    }

    let items = if opts.files_only { tally.files_with_matches } else { tally.total_matches };
    let mut report = GrepReport {
        files,
        total_matches: tally.total_matches,
        files_with_matches: tally.files_with_matches,
        files_searched,
        truncated: tally.shown < items,
        backend: backend.to_owned(),
        note: None,
        estimated_tokens_saved: 0,
    };
    let rendered = render(&report, opts.max_tokens);
    report.estimated_tokens_saved =
        estimate_tokens(tally.raw_bytes).saturating_sub(estimate_tokens(rendered.len()));
    report
}

/// Files-only mode: one shown item per file, empty match list.
fn take_listing(
    opts: &GrepOptions,
    budget: usize,
    tally: &mut Tally,
    path: &str,
) -> Option<Vec<LineMatch>> {
    let cost = path.len() + ROW_OVERHEAD;
    if tally.shown >= opts.max_matches || (tally.shown > 0 && tally.used + cost > budget) {
        return None;
    }
    tally.shown += 1;
    tally.used += cost;
    Some(Vec::new())
}

/// Keeps the matches of one file that fit the per-file, total and token caps.
fn take_matches(
    opts: &GrepOptions,
    budget: usize,
    tally: &mut Tally,
    hit: &FileHits,
) -> Option<Vec<LineMatch>> {
    let mut kept: Vec<LineMatch> = Vec::new();
    for m in hit.matches.iter().take(opts.max_per_file) {
        let header = if kept.is_empty() { hit.path.len() + 1 } else { 0 };
        let cost = row_cost(m) + header;
        if tally.shown >= opts.max_matches || (tally.shown > 0 && tally.used + cost > budget) {
            break;
        }
        tally.shown += 1;
        tally.used += cost;
        kept.push(m.clone());
    }
    if kept.is_empty() { None } else { Some(kept) }
}

fn row_cost(m: &LineMatch) -> usize {
    let context: usize = m.before.iter().chain(&m.after).map(|(_, t)| t.len() + ROW_OVERHEAD).sum();
    m.text.len() + ROW_OVERHEAD + context
}
