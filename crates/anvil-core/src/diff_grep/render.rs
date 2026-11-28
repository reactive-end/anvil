//! Compact text rendering of a [`GrepReport`], grouped by file.

use crate::diff_grep::grep::{FileMatches, GrepReport};

const CHARS_PER_TOKEN: usize = 4;

/// One printable row of a file block.
struct Row<'a> {
    line: u32,
    is_match: bool,
    text: &'a str,
    symbol: Option<&'a str>,
}

/// Renders `report` grouped by file within roughly `max_tokens`, ending with a totals footer.
///
/// Layout: a file header, indented `line: text  [symbol]` rows (`line- text` for context lines,
/// `--` between non-adjacent groups), `(+N more in this file)` when capped, then a `# grep:` footer.
pub fn render(report: &GrepReport, max_tokens: usize) -> String {
    let budget = max_tokens.saturating_mul(CHARS_PER_TOKEN);
    let mut out = String::new();
    let mut shown = 0usize;
    let mut cut = false;

    'files: for file in &report.files {
        for (line, is_match) in file_lines(file) {
            if !out.is_empty() && out.len() + line.len() + 1 > budget {
                cut = true;
                break 'files;
            }
            out.push_str(&line);
            out.push('\n');
            shown += usize::from(is_match);
        }
    }

    out.push_str(&footer(report, shown, cut));
    if let Some(note) = &report.note {
        out.push_str(&format!("\n# note: {note}"));
    }
    out
}

fn footer(report: &GrepReport, shown: usize, cut: bool) -> String {
    let listing = report.files.iter().all(|f| f.matches.is_empty());
    let count = if report.total_matches == 0 {
        "no matches".to_owned()
    } else if listing {
        format!("{} match(es) in {} file(s)", report.total_matches, report.files_with_matches)
    } else {
        format!(
            "{shown} of {} match(es) in {} file(s)",
            report.total_matches, report.files_with_matches
        )
    };
    let truncated = if report.truncated || cut { ", truncated" } else { "" };
    format!(
        "# grep: {count}, searched {} file(s), backend={}{truncated}",
        report.files_searched, report.backend
    )
}

/// Builds the printable lines of one file, flagging which of them are match rows.
fn file_lines(file: &FileMatches) -> Vec<(String, bool)> {
    if file.matches.is_empty() {
        return vec![(format!("{} ({})", file.path, file.total), false)];
    }
    let rows = collect_rows(file);
    let has_context = rows.iter().any(|r| !r.is_match);
    let mut lines = vec![(file.path.clone(), false)];
    let mut previous: Option<u32> = None;
    for row in &rows {
        if has_context && previous.is_some_and(|p| row.line > p.saturating_add(1)) {
            lines.push(("  --".to_owned(), false));
        }
        previous = Some(row.line);
        lines.push((format_row(row), row.is_match));
    }
    let hidden = file.total.saturating_sub(file.matches.len());
    if hidden > 0 {
        lines.push((format!("  (+{hidden} more in this file)"), false));
    }
    lines
}

fn collect_rows(file: &FileMatches) -> Vec<Row<'_>> {
    let mut rows: Vec<Row<'_>> = Vec::new();
    for m in &file.matches {
        let context = m.before.iter().chain(&m.after);
        rows.extend(context.map(|(line, text)| Row {
            line: *line,
            is_match: false,
            text,
            symbol: None,
        }));
        rows.push(Row { line: m.line, is_match: true, text: &m.text, symbol: m.symbol.as_deref() });
    }
    rows.sort_by(|a, b| a.line.cmp(&b.line).then(b.is_match.cmp(&a.is_match)));
    rows.dedup_by(|later, earlier| later.line == earlier.line);
    rows
}

fn format_row(row: &Row<'_>) -> String {
    if !row.is_match {
        return format!("  {}- {}", row.line, row.text);
    }
    match row.symbol {
        Some(symbol) => format!("  {}: {}  [{symbol}]", row.line, row.text),
        None => format!("  {}: {}", row.line, row.text),
    }
}
