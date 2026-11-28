//! Parser for the ripgrep-compatible `--json` stream that `tgrep` prints (begin/match/context/end/summary).

use anyhow::{Context, Result};
use serde_json::Value;

/// Matches and context lines `tgrep` reported for one file, with raw (unnormalized) path text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ParsedFile {
    pub path: String,
    pub matches: Vec<(u32, String)>,
    pub context: Vec<(u32, String)>,
}

/// Everything extracted from one `tgrep --json` run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ParsedStream {
    pub files: Vec<ParsedFile>,
    /// `stats.searches` from the summary record when present.
    pub searches: Option<usize>,
}

/// Parses a JSON-lines stream; blank lines are skipped and malformed JSON is an error.
pub(crate) fn parse_json_stream(input: &str) -> Result<ParsedStream> {
    let mut stream = ParsedStream::default();
    let mut current: Option<ParsedFile> = None;

    for (index, raw) in input.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(raw)
            .with_context(|| format!("invalid tgrep JSON on line {}", index + 1))?;
        let data = &record["data"];
        match record["type"].as_str() {
            Some("begin") => {
                if let Some(done) = current.take() {
                    stream.files.push(done);
                }
                current =
                    text_of(&data["path"]).map(|path| ParsedFile { path, ..Default::default() });
            }
            Some("match") => push_line(&mut current, data, true),
            Some("context") => push_line(&mut current, data, false),
            Some("end") => {
                if let Some(done) = current.take() {
                    stream.files.push(done);
                }
            }
            Some("summary") => {
                let searches = &data["stats"]["searches"];
                stream.searches = searches.as_u64().and_then(|n| usize::try_from(n).ok());
            }
            _ => {}
        }
    }
    if let Some(done) = current.take() {
        stream.files.push(done);
    }
    Ok(stream)
}

fn push_line(current: &mut Option<ParsedFile>, data: &Value, is_match: bool) {
    let Some(text) = text_of(&data["lines"]) else {
        return;
    };
    let Some(line) = data["line_number"].as_u64().and_then(|n| u32::try_from(n).ok()) else {
        return;
    };
    if current.is_none() {
        *current = text_of(&data["path"]).map(|path| ParsedFile { path, ..Default::default() });
    }
    if let Some(file) = current {
        if is_match {
            file.matches.push((line, text));
        } else {
            file.context.push((line, text));
        }
    }
}

/// Returns the UTF-8 `text` of a ripgrep `{ "text": ... }` object; `{ "bytes": ... }` yields `None`.
fn text_of(value: &Value) -> Option<String> {
    value["text"].as_str().map(str::to_owned)
}
