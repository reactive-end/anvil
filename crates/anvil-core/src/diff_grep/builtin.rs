//! Built-in search engine: `ignore` walker plus ripgrep's `grep-regex` and `grep-searcher`.

mod sink;
mod walk;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use anyhow::{Context, Result, anyhow};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder};

use crate::diff_grep::grep::{FileHits, GrepOptions, GrepReport, assemble};
use sink::FileSink;
use walk::Candidate;

pub(crate) use sink::clean_line;

const MAX_THREADS: usize = 8;
const FILES_PER_WORKER: usize = 16;

/// Runs the built-in engine over `root`.
pub(crate) fn run(root: &Path, opts: &GrepOptions) -> Result<GrepReport> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(opts.ignore_case)
        .case_smart(opts.smart_case && !opts.ignore_case)
        .fixed_strings(opts.fixed_strings)
        .word(opts.word)
        .line_terminator(Some(b'\n'))
        .build(&opts.pattern)
        .with_context(|| format!("invalid grep pattern: {}", opts.pattern))?;
    let files = walk::collect_files(root, opts)?;
    let results = search_all(&files, &matcher, opts)?;
    let files_searched = results.iter().flatten().count();
    let hits = results.into_iter().flatten().collect();
    Ok(assemble(opts, hits, files_searched, "builtin"))
}

fn build_searcher(opts: &GrepOptions) -> Searcher {
    SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(0))
        .line_number(true)
        .before_context(opts.context)
        .after_context(opts.context)
        .build()
}

/// Searches every candidate (in parallel for large sets); the result keeps candidate order.
fn search_all(
    files: &[Candidate],
    matcher: &RegexMatcher,
    opts: &GrepOptions,
) -> Result<Vec<Option<FileHits>>> {
    let workers = thread::available_parallelism()
        .map_or(1, usize::from)
        .min(MAX_THREADS)
        .min(files.len().div_ceil(FILES_PER_WORKER))
        .max(1);
    let mut slots: Vec<Option<FileHits>> = Vec::new();
    slots.resize_with(files.len(), || None);
    if workers == 1 {
        let mut searcher = build_searcher(opts);
        for (slot, file) in slots.iter_mut().zip(files) {
            *slot = search_one(file, matcher, &mut searcher, opts);
        }
        return Ok(slots);
    }

    let next = AtomicUsize::new(0);
    thread::scope(|scope| -> Result<()> {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut searcher = build_searcher(opts);
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(file) = files.get(index) else {
                            break;
                        };
                        done.push((index, search_one(file, matcher, &mut searcher, opts)));
                    }
                    done
                })
            })
            .collect();
        for handle in handles {
            let batch = handle.join().map_err(|_| anyhow!("search worker panicked"))?;
            for (index, hits) in batch {
                slots[index] = hits;
            }
        }
        Ok(())
    })?;
    Ok(slots)
}

fn search_one(
    file: &Candidate,
    matcher: &RegexMatcher,
    searcher: &mut Searcher,
    opts: &GrepOptions,
) -> Option<FileHits> {
    let cap = if opts.files_only { 0 } else { opts.max_per_file };
    let mut sink = FileSink::new(cap);
    searcher.search_path(matcher, &file.path, &mut sink).ok()?;
    sink.into_hits(file.rel.clone())
}

#[cfg(test)]
mod tests;
