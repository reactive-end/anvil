//! Optional Microsoft `tgrep` backend: trigram-indexed, ripgrep-compatible search driven through `--json`.
//!
//! Anvil never builds a `.tgrep/` index or starts `tgrep serve`; it only uses what the user already runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};

use crate::diff_grep::builtin::clean_line;
use crate::diff_grep::grep::{FileHits, GrepOptions, GrepReport, LineMatch, assemble};
use crate::diff_grep::is_noise_file;
use crate::diff_grep::paths::{TGREP_EXCLUDE_GLOBS, is_searchable_path};
use crate::diff_grep::tgrep_parse::{ParsedFile, ParsedStream, parse_json_stream};
use crate::fs_util::{bin_in_path, relativize_path};
use crate::proc_util::run_command_with_timeout;

const STATUS_TIMEOUT: Duration = Duration::from_secs(2);
const SEARCH_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_FILE_SIZE_FLAG: &str = "512K";

/// Locates the `tgrep` executable on `PATH`.
pub fn find_binary() -> Option<PathBuf> {
    bin_in_path("tgrep")
}

/// `auto` backend rule: binary present, `<root>/.tgrep/` exists, and `tgrep status` reports a live watched server.
pub fn auto_eligible(root: &Path, binary: Option<&Path>) -> bool {
    let Some(binary) = binary else {
        return false;
    };
    if !root.join(".tgrep").is_dir() {
        return false;
    }
    let mut cmd = Command::new(binary);
    cmd.arg("status").current_dir(root);
    run_command_with_timeout(&mut cmd, STATUS_TIMEOUT).is_ok_and(|out| {
        !out.timed_out
            && out.status_code == 0
            && status_shows_fresh_server(&String::from_utf8_lossy(&out.stdout))
    })
}

/// True when `tgrep status` output describes a running server with an active file watcher.
///
/// A stopped server prints `Server:     not running` after the index summary instead of a
/// `Server status for ...` block; a polling-only or still-indexing server is not considered fresh.
pub fn status_shows_fresh_server(output: &str) -> bool {
    let field = |name: &str| {
        output.lines().find_map(|line| line.trim().strip_prefix(name).map(|v| v.trim().to_owned()))
    };
    output.trim_start().starts_with("Server status for")
        && field("Watcher:").is_some_and(|v| v.starts_with("active"))
        && field("Indexing:").is_none_or(|v| v.starts_with("complete"))
        && field("Reconcile overdue:").is_none_or(|v| v.starts_with("no"))
}

/// Runs `tgrep --json` and converts its stream into a report; any failure is an `Err` for fallback.
pub(crate) fn run(root: &Path, opts: &GrepOptions, binary: Option<&Path>) -> Result<GrepReport> {
    let binary = binary.ok_or_else(|| anyhow!("`tgrep` is not on PATH"))?;
    let mut cmd = Command::new(binary);
    cmd.args(build_args(root, opts)).current_dir(root);
    let out = run_command_with_timeout(&mut cmd, SEARCH_TIMEOUT)?;
    if out.timed_out {
        bail!("tgrep timed out after {}s", SEARCH_TIMEOUT.as_secs());
    }
    if out.status_code != 0 && out.status_code != 1 {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("no error output");
        bail!("tgrep exited with code {}: {}", out.status_code, detail.trim());
    }
    let stream = parse_json_stream(&String::from_utf8_lossy(&out.stdout))?;
    Ok(build_report(root, opts, stream))
}

/// Builds the `tgrep` argument list equivalent to `opts`.
pub(crate) fn build_args(root: &Path, opts: &GrepOptions) -> Vec<String> {
    let mut args: Vec<String> =
        ["--json", "--no-config", "--color", "never", "--max-filesize", MAX_FILE_SIZE_FLAG]
            .map(str::to_owned)
            .into();
    if opts.ignore_case {
        args.push("-i".to_owned());
    } else if opts.smart_case {
        args.push("-S".to_owned());
    } else {
        args.push("-s".to_owned());
    }
    for (enabled, flag) in [(opts.fixed_strings, "-F"), (opts.word, "-w")] {
        if enabled {
            args.push(flag.to_owned());
        }
    }
    // Always `--hidden`: tgrep would skip `.github/` otherwise. Dot-entries Anvil does not search by
    // default are dropped from the results afterwards by `is_searchable_path`.
    args.push("--hidden".to_owned());
    for glob in TGREP_EXCLUDE_GLOBS.iter().copied().chain(opts.globs.iter().map(String::as_str)) {
        args.extend(["-g".to_owned(), glob.to_owned()]);
    }
    if opts.context > 0 {
        args.extend(["-C".to_owned(), opts.context.to_string()]);
    }
    let index = root.join(".tgrep");
    if index.is_dir() {
        args.extend(["--index-path".to_owned(), index.to_string_lossy().into_owned()]);
    }
    args.push("--".to_owned());
    args.push(opts.pattern.clone());
    args.extend(opts.paths.iter().cloned());
    args
}

/// Applies Anvil's filters, deterministic ordering and caps to a parsed `tgrep` stream.
pub(crate) fn build_report(root: &Path, opts: &GrepOptions, stream: ParsedStream) -> GrepReport {
    let mut hits: Vec<FileHits> =
        stream.files.into_iter().filter_map(|file| to_hits(root, opts, file)).collect();
    hits.sort_by(|a, b| a.path.cmp(&b.path));
    hits.dedup_by(|a, b| a.path == b.path);
    let files_searched = stream.searches.filter(|n| *n > 0).unwrap_or(hits.len()).max(hits.len());
    assemble(opts, hits, files_searched, "tgrep")
}

fn to_hits(root: &Path, opts: &GrepOptions, file: ParsedFile) -> Option<FileHits> {
    let rel = relativize_path(root, Path::new(&file.path));
    let rel = rel.strip_prefix("./").unwrap_or(&rel).to_owned();
    if is_noise_file(&rel) || !is_searchable_path(&rel, opts.hidden) || file.matches.is_empty() {
        return None;
    }
    let cap = if opts.files_only { 0 } else { opts.max_per_file };
    let raw_bytes = file.matches.iter().map(|(_, t)| t.len()).sum();
    let mut context = attach_context(&file, opts.context);
    let matches = file
        .matches
        .iter()
        .take(cap)
        .map(|(line, text)| {
            let (before, after) = context.remove(line).unwrap_or_default();
            LineMatch {
                line: *line,
                text: clean_line(text.as_bytes()),
                before,
                after,
                symbol: None,
            }
        })
        .collect();
    Some(FileHits { path: rel, total: file.matches.len(), matches, raw_bytes })
}

type MatchContext = (Vec<(u32, String)>, Vec<(u32, String)>);

/// Assigns each context line to the match it belongs to: trailing context of the nearest earlier
/// match within `distance`, otherwise leading context of the next match within `distance`.
fn attach_context(file: &ParsedFile, distance: usize) -> BTreeMap<u32, MatchContext> {
    let mut lines: Vec<u32> = file.matches.iter().map(|(line, _)| *line).collect();
    lines.sort_unstable();
    let within = |a: u32, b: u32| (a.abs_diff(b) as usize) <= distance;
    let mut attached: BTreeMap<u32, MatchContext> = BTreeMap::new();
    for (line, text) in &file.context {
        let split = lines.partition_point(|m| m < line);
        let previous = split.checked_sub(1).map(|i| lines[i]).filter(|m| within(*m, *line));
        let next = lines.get(split).copied().filter(|m| within(*m, *line));
        let entry = (*line, clean_line(text.as_bytes()));
        if let Some(owner) = previous {
            attached.entry(owner).or_default().1.push(entry);
        } else if let Some(owner) = next {
            attached.entry(owner).or_default().0.push(entry);
        }
    }
    attached
}

#[cfg(test)]
mod tests;
