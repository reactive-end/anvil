//! Anti-reward-hacking guard detecting test skipping or weakened assertions in test diffs.

use crate::traits::{Diagnostic, Severity};

const SKIP_MARKERS: &[&str] = &[
    ".skip(",
    ".only(",
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
    "assertTrue(",
    "assertFalse(",
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
        || name.ends_with("test.kt")
        || name.ends_with("test.php")
        || (name.starts_with("test_") && name.ends_with(".py"))
}

/// Inspects a unified diff of a test file for skipped tests or stripped assertions.
pub fn check_test_tampering_diff(rel_path: &str, unified_diff: &str) -> Vec<Diagnostic> {
    if !is_test_file(rel_path) || unified_diff.trim().is_empty() {
        return Vec::new();
    }
    let mut added = Vec::new();
    let mut removed = Vec::new();
    for line in unified_diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if let Some(rest) = line.strip_prefix('+') {
            added.push(rest);
        } else if let Some(rest) = line.strip_prefix('-') {
            removed.push(rest);
        }
    }

    let allow_override = added
        .iter()
        .any(|l| has_valid_allow_comment(l, "no-test-tampering"));
    if allow_override {
        return Vec::new();
    }

    let mut diags = collect_skip_tampering(rel_path, &added);
    let removed_asserts = count_assertions(&removed);
    let added_asserts = count_assertions(&added);
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

fn collect_skip_tampering(rel_path: &str, added: &[&str]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for line in added {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }
        if let Some(marker) = SKIP_MARKERS.iter().find(|&&m| line.contains(m)) {
            out.push(Diagnostic {
                rule: "no-test-tampering".to_owned(),
                severity: Severity::Error,
                file: rel_path.to_owned(),
                line: 1,
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

fn count_assertions(lines: &[&str]) -> usize {
    lines
        .iter()
        .filter(|l| {
            let trimmed = l.trim();
            !trimmed.starts_with("//") && !trimmed.starts_with('#')
        })
        .map(|l| {
            ASSERTION_MARKERS
                .iter()
                .map(|m| l.matches(m).count())
                .sum::<usize>()
        })
        .sum()
}
