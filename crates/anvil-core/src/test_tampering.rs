//! Anti-reward-hacking guard detecting test skipping or weakened assertions in test diffs.

use crate::traits::{Diagnostic, Severity};

const SKIP_MARKERS: &[&str] = &[
    "it.skip(",
    "test.skip(",
    "describe.skip(",
    "context.skip(",
    "suite.skip(",
    "fixture.skip(",
    "specify.skip(",
    "it.only(",
    "test.only(",
    "describe.only(",
    "context.only(",
    "suite.only(",
    "xit(",
    "xdescribe(",
    "xtest(",
    "#[ignore",
    "@pytest.mark.skip",
    "@unittest.skip",
    "@Disabled",
    "markTestSkipped(",
];

const ASSERTION_MARKERS: &[&str] = &[
    "assert_eq!",
    "assert_ne!",
    "assert!",
    "assert(",
    "expect(",
    "assertEquals(",
    "assertNotEquals(",
    "assertSame(",
    "assertCount(",
    "assertNull(",
    "assertNotNull(",
    "assertTrue(",
    "assertFalse(",
    "assert_eq(",
    "assert_ne(",
    "assert_true(",
    "assert_false(",
    "assert_ok(",
    "should(",
    "verify(",
];

/// Returns `true` when `rel_path` matches common unit/integration test file naming conventions.
pub fn is_test_file(rel_path: &str) -> bool {
    let norm = rel_path.replace('\\', "/").to_ascii_lowercase();
    let name = norm.rsplit('/').next().unwrap_or(&norm);
    norm.starts_with("tests/")
        || norm.starts_with("test/")
        || norm.contains("/tests/")
        || norm.contains("/test/")
        || norm.contains("/__tests__/")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.ends_with("_test.rs")
        || name.ends_with("_tests.rs")
        || name.ends_with("_test.py")
        || name.ends_with("_test.dart")
        || name.ends_with("_test.gd")
        || name.ends_with("test.kt")
        || name.ends_with("test.php")
        || (name.starts_with("test_") && name.ends_with(".py"))
}

/// Inspects a unified diff of a test file for skipped tests or stripped assertions.
pub fn check_test_tampering_diff(rel_path: &str, unified_diff: &str) -> Vec<Diagnostic> {
    if !is_test_file(rel_path) || unified_diff.trim().is_empty() {
        return Vec::new();
    }
    let (added, removed) = split_diff_lines(unified_diff);
    let allow_override = added.iter().any(|(_, l)| has_valid_allow_comment(l, "no-test-tampering"));
    if allow_override {
        return Vec::new();
    }

    let mut diags = collect_skip_tampering(rel_path, &added);
    let removed_asserts = count_assertions(removed.iter().copied());
    let added_asserts = count_assertions(added.iter().map(|(_, l)| *l));
    if removed_asserts >= 2 && removed_asserts > added_asserts {
        diags.push(Diagnostic {
            rule: "no-test-tampering".to_owned(),
            severity: Severity::Error,
            file: rel_path.to_owned(),
            line: 1,
            column: 1,
            message: format!(
                "test diff removed {removed_asserts} assertion(s) while adding only {added_asserts}"
            ),
            suggestion: Some(
                "Keep test assertions intact or annotate intentional test updates with `anvil-allow(no-test-tampering): <reason>`."
                    .to_owned(),
            ),
        });
    }
    diags
}

/// Splits a unified diff into added lines (with post-image line numbers) and removed lines.
fn split_diff_lines(unified_diff: &str) -> (Vec<(u32, &str)>, Vec<&str>) {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut line_no = 1u32;
    for line in unified_diff.lines() {
        if line.starts_with("@@") {
            line_no = parse_hunk_start(line).unwrap_or(1);
        } else if line.starts_with("+++") || line.starts_with("---") {
            continue;
        } else if let Some(rest) = line.strip_prefix('+') {
            added.push((line_no, rest));
            line_no += 1;
        } else if let Some(rest) = line.strip_prefix('-') {
            removed.push(rest);
        } else {
            line_no += 1;
        }
    }
    (added, removed)
}

fn parse_hunk_start(header: &str) -> Option<u32> {
    let post_image = header.split_whitespace().find_map(|tok| tok.strip_prefix('+'))?;
    post_image.split(',').next()?.parse().ok()
}

/// Blanks string-literal contents so test fixtures quoting a marker are not mistaken for code.
fn strip_string_literals(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.char_indices();
    while let Some((idx, ch)) = chars.next() {
        out.push(ch);
        if !matches!(ch, '"' | '\'' | '`') || closing_quote(&line[idx + 1..], ch).is_none() {
            continue;
        }
        let mut escaped = false;
        for (_, inner) in chars.by_ref() {
            if inner == ch && !escaped {
                out.push(ch);
                break;
            }
            escaped = inner == '\\' && !escaped;
        }
    }
    out
}

fn closing_quote(rest: &str, quote: char) -> Option<usize> {
    let mut escaped = false;
    for (idx, ch) in rest.char_indices() {
        if ch == quote && !escaped {
            return Some(idx);
        }
        escaped = ch == '\\' && !escaped;
    }
    None
}

fn collect_skip_tampering(rel_path: &str, added: &[(u32, &str)]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for (line_no, line) in added {
        let trimmed = line.trim();
        let is_rust_attr = trimmed.starts_with("#[") || trimmed.starts_with("#![");
        if trimmed.starts_with("//") || (trimmed.starts_with('#') && !is_rust_attr) {
            continue;
        }
        let code = strip_string_literals(line);
        if let Some(marker) = SKIP_MARKERS.iter().find(|&&m| code.contains(m)) {
            out.push(Diagnostic {
                rule: "no-test-tampering".to_owned(),
                severity: Severity::Error,
                file: rel_path.to_owned(),
                line: *line_no,
                column: 1,
                message: format!("test-skipping marker `{marker}` introduced in test diff"),
                suggestion: Some(
                    "Fix the failing test instead of skipping it, or use `anvil-allow(no-test-tampering): <reason>`."
                        .to_owned(),
                ),
            });
            break;
        }
    }
    out
}

fn has_valid_allow_comment(line: &str, rule: &str) -> bool {
    let trimmed = line.trim();
    let is_comment = trimmed.starts_with("//")
        || trimmed.starts_with('#')
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*');
    if !is_comment {
        return false;
    }
    let marker = format!("anvil-allow({rule}):");
    let Some((_, rest)) = trimmed.split_once(&marker) else {
        return false;
    };
    let reason = rest.trim().trim_end_matches("*/").trim();
    reason.chars().filter(|c| !c.is_whitespace()).count() >= 3
}

fn count_assertions<'a>(lines: impl Iterator<Item = &'a str>) -> usize {
    lines
        .filter(|l| {
            let trimmed = l.trim();
            !trimmed.starts_with("//") && !trimmed.starts_with('#')
        })
        .map(strip_string_literals)
        .map(|l| {
            let marker_hits: usize = ASSERTION_MARKERS.iter().map(|m| l.matches(m).count()).sum();
            let py_assert = usize::from(l.trim_start().starts_with("assert "));
            marker_hits + py_assert
        })
        .sum()
}
