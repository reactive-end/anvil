//! Per-file match collector for `grep-searcher`: exact counts, capped matches, context lines.

use std::io;

use grep_searcher::{Searcher, Sink, SinkContext, SinkContextKind, SinkFinish, SinkMatch};

use crate::diff_grep::grep::{FileHits, LineMatch};

/// Longest line kept in a report before it is cut with an ellipsis.
const MAX_LINE_CHARS: usize = 240;

/// Counts every matching line but only keeps the first `cap` of them with their context.
pub(super) struct FileSink {
    cap: usize,
    total: usize,
    raw_bytes: usize,
    matches: Vec<LineMatch>,
    pending_before: Vec<(u32, String)>,
    last_collected: bool,
    binary: bool,
}

impl FileSink {
    pub(super) fn new(cap: usize) -> Self {
        Self {
            cap,
            total: 0,
            raw_bytes: 0,
            matches: Vec::new(),
            pending_before: Vec::new(),
            last_collected: false,
            binary: false,
        }
    }

    /// Returns the collected hits, or `None` when the file turned out to be binary.
    pub(super) fn into_hits(self, path: String) -> Option<FileHits> {
        if self.binary {
            return None;
        }
        Some(FileHits { path, total: self.total, matches: self.matches, raw_bytes: self.raw_bytes })
    }
}

impl Sink for FileSink {
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, io::Error> {
        self.total += 1;
        self.raw_bytes += mat.bytes().len();
        if self.matches.len() < self.cap {
            self.matches.push(LineMatch {
                line: line_number(mat.line_number()),
                text: clean_line(mat.bytes()),
                before: std::mem::take(&mut self.pending_before),
                after: Vec::new(),
                symbol: None,
            });
            self.last_collected = true;
        } else {
            self.pending_before.clear();
            self.last_collected = false;
        }
        Ok(true)
    }

    fn context(&mut self, _searcher: &Searcher, ctx: &SinkContext<'_>) -> Result<bool, io::Error> {
        let entry = (line_number(ctx.line_number()), clean_line(ctx.bytes()));
        match ctx.kind() {
            SinkContextKind::Before if self.matches.len() < self.cap => {
                self.pending_before.push(entry);
            }
            SinkContextKind::After if self.last_collected => {
                if let Some(last) = self.matches.last_mut() {
                    last.after.push(entry);
                }
            }
            _ => {}
        }
        Ok(true)
    }

    fn finish(&mut self, _searcher: &Searcher, finish: &SinkFinish) -> Result<(), io::Error> {
        self.binary = finish.binary_byte_offset().is_some();
        Ok(())
    }
}

fn line_number(raw: Option<u64>) -> u32 {
    raw.map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
}

/// Decodes a line leniently, trims it, and cuts it at [`MAX_LINE_CHARS`] characters.
pub(crate) fn clean_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    match trimmed.char_indices().nth(MAX_LINE_CHARS) {
        Some((cut, _)) => format!("{}...", &trimmed[..cut]),
        None => trimmed.to_owned(),
    }
}
