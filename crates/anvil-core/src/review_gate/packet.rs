//! The review packet: a compact, deterministic digest of one task's change for the orchestrator.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::diff_grep::compress_diff;
use crate::git::{self, batched_working_tree_diffs};
use crate::stats::load_stats;
use crate::test_tampering::{assertion_delta, is_test_file};

const MAX_DIFF_LINES: usize = 400;
const MAX_DIFF_TOKENS: usize = 4000;

const CHECKLIST: &[&str] = &[
    "It does what the task asked and nothing more: no unrelated edits, no speculative code.",
    "Names and signatures say what they do; public contracts are documented.",
    "Errors are handled or propagated, never swallowed; empty, missing, and large inputs behave.",
    "Tests exercise the new behavior and would fail without it; no assertion was weakened.",
    "No duplicated logic or dead code; existing helpers are reused.",
    "`anvil check` passed after the last edit.",
];

/// Builds the packet text for `task_id`'s `files`: per-file line counts, test assertion changes,
/// the last check result, a review checklist, and the token-capped diff.
pub fn build_packet(
    repo_root: &Path,
    task_id: &str,
    title: &str,
    files: &[String],
) -> Result<String> {
    let diffs = batched_working_tree_diffs(repo_root)?;
    let untracked = git::working_tree_status(repo_root).map(|s| s.untracked).unwrap_or_default();
    let mut rows = Vec::new();
    let mut combined = String::new();
    let (mut plus, mut minus) = (0usize, 0usize);
    for rel in files {
        let diff = match diffs.get(rel) {
            Some(diff) => diff.clone(),
            None if untracked.contains(rel) => new_file_diff(repo_root, rel),
            None => String::new(),
        };
        let (added, removed) = line_counts(&diff);
        plus += added;
        minus += removed;
        rows.push(file_row(repo_root, rel, &diff, added, removed));
        combined.push_str(&diff);
    }
    let mut out = String::new();
    let _ = writeln!(out, "Review packet · {task_id} · {title}");
    let _ = writeln!(
        out,
        "{} file(s) · +{plus} -{minus} · last anvil check: {}",
        files.len(),
        last_check(repo_root)
    );
    out.push_str("\nFiles\n");
    for row in rows {
        let _ = writeln!(out, "  {row}");
    }
    out.push_str("\nCheck each before approving\n");
    for (n, item) in CHECKLIST.iter().enumerate() {
        let _ = writeln!(out, "  {}. {item}", n + 1);
    }
    let compressed = compress_diff(&combined, MAX_DIFF_LINES, MAX_DIFF_TOKENS);
    if !compressed.text.is_empty() {
        let _ = write!(out, "\nDiff\n{}\n", compressed.text.trim_end());
    }
    let _ = write!(
        out,
        "\nApprove: anvil review approve {task_id} \"<what you verified>\" (MCP review_approve).\n\
         Problems: send them back to the executor, log each with review_log, then build a new packet.\n"
    );
    Ok(out)
}

fn file_row(repo_root: &Path, rel: &str, diff: &str, added: usize, removed: usize) -> String {
    let state = if !repo_root.join(rel).exists() {
        "D"
    } else if diff.contains("\nnew file mode") || diff.starts_with("new file") {
        "A"
    } else if diff.is_empty() {
        "="
    } else {
        "M"
    };
    let mut row = format!("{state} {rel}  +{added} -{removed}");
    if is_test_file(rel) && !diff.is_empty() {
        let (asserts_added, asserts_removed) = assertion_delta(diff);
        let _ = write!(row, "  assertions +{asserts_added} -{asserts_removed}");
    }
    row
}

/// A unified diff that adds every line of an untracked file.
fn new_file_diff(repo_root: &Path, rel: &str) -> String {
    let Ok(text) = fs::read_to_string(repo_root.join(rel)) else {
        return String::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut diff = format!(
        "diff --git a/{rel} b/{rel}\nnew file mode 100644\n--- /dev/null\n+++ b/{rel}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in lines {
        let _ = writeln!(diff, "+{line}");
    }
    diff
}

fn line_counts(diff: &str) -> (usize, usize) {
    diff.lines().filter(|line| !line.starts_with("+++") && !line.starts_with("---")).fold(
        (0, 0),
        |(added, removed), line| match line.as_bytes().first() {
            Some(b'+') => (added + 1, removed),
            Some(b'-') => (added, removed + 1),
            _ => (added, removed),
        },
    )
}

fn last_check(repo_root: &Path) -> &'static str {
    match load_stats(repo_root).ok().and_then(|stats| stats.quality.last_check_passed) {
        Some(true) => "PASS",
        Some(false) => "FAIL",
        None => "not run",
    }
}

#[cfg(test)]
mod tests {
    use super::line_counts;

    #[test]
    fn counts_changed_lines_without_headers() {
        let diff = "--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-old\n+new\n+more\n same\n";
        assert_eq!(line_counts(diff), (2, 1));
    }
}
