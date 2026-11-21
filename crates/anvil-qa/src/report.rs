//! Token-budgeted compressed before/after QA report formatting for LLM consumption.

use std::fmt::Write as _;

use crate::model::{QaReport, QaViolation, UiPlatform, ViewportCapture};

/// Line and approximate-token limits for compressed QA reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportBudget {
    pub max_lines: usize,
    pub max_tokens: usize,
}

impl Default for ReportBudget {
    fn default() -> Self {
        Self {
            max_lines: 50,
            max_tokens: 1200,
        }
    }
}

/// Aggregates before/after web captures and mobile violations into a [`QaReport`].
pub fn build_report(
    platform: UiPlatform,
    before: Vec<ViewportCapture>,
    after: Vec<ViewportCapture>,
    mobile_violations: Vec<QaViolation>,
    notes: Vec<String>,
) -> QaReport {
    let web_captures = if after.is_empty() { &before } else { &after };
    let web_passed = web_captures.iter().all(ViewportCapture::passed);
    let passed = web_passed && mobile_violations.is_empty();

    QaReport {
        platform,
        passed,
        before,
        after,
        mobile_violations,
        notes,
    }
}

/// Formats a [`QaReport`] into a deterministic, token-budgeted summary string.
pub fn format_compressed_report(report: &QaReport, budget: ReportBudget) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(budget.max_lines.min(64));
    let before_web: usize = report.before.iter().map(ViewportCapture::total_violations).sum();
    let after_web: usize = report.after.iter().map(ViewportCapture::total_violations).sum();
    let active_web = if report.after.is_empty() { before_web } else { after_web };
    let status = if report.passed { "PASS" } else { "FAIL" };

    lines.push(format!(
        "QA [{status}] platform={} web_violations={active_web} (before={before_web}) mobile_violations={}",
        platform_str(report.platform),
        report.mobile_violations.len()
    ));

    append_viewport_deltas(report, &mut lines);
    append_notes(&report.notes, &mut lines);

    let active_captures = if report.after.is_empty() {
        &report.before
    } else {
        &report.after
    };
    let all_violations: Vec<&QaViolation> = active_captures
        .iter()
        .flat_map(|c| c.violations.iter())
        .chain(report.mobile_violations.iter())
        .collect();

    append_budgeted_violations(&all_violations, &mut lines, budget);
    lines.join("\n")
}

fn append_viewport_deltas(report: &QaReport, lines: &mut Vec<String>) {
    if report.after.is_empty() {
        for cap in &report.before {
            lines.push(format_single_viewport("baseline", cap, None));
        }
        return;
    }

    for after in &report.after {
        let before_count = report
            .before
            .iter()
            .find(|b| b.name == after.name)
            .map(ViewportCapture::total_violations);
        lines.push(format_single_viewport("after", after, before_count));
    }
}

fn format_single_viewport(
    phase: &str,
    cap: &ViewportCapture,
    before_count: Option<usize>,
) -> String {
    let mut line = format!(
        "- {} ({}x{}): ",
        cap.name, cap.width, cap.height
    );
    let current = cap.total_violations();
    if let Some(prev) = before_count {
        let _ = write!(line, "before={prev} -> {phase}={current}");
    } else {
        let _ = write!(line, "{phase}={current}");
    }
    let _ = write!(line, " [{}]", if cap.passed() { "PASS" } else { "FAIL" });
    if let Some(path) = &cap.screenshot_path {
        let _ = write!(line, " shot={}", path.display());
    }
    line
}

fn append_notes(notes: &[String], lines: &mut Vec<String>) {
    for note in notes {
        lines.push(format!("- note: {note}"));
    }
}

fn append_budgeted_violations(
    violations: &[&QaViolation],
    lines: &mut Vec<String>,
    budget: ReportBudget,
) {
    let char_budget = budget.max_tokens.saturating_mul(4);
    let mut used_chars: usize = lines.iter().map(|l| l.len() + 1).sum();

    for (idx, v) in violations.iter().enumerate() {
        let formatted = format_violation_line(v);
        let next_chars = used_chars + formatted.len() + 1;
        let reserve_tail = idx + 1 < violations.len();
        let line_limit = if reserve_tail {
            budget.max_lines.saturating_sub(1)
        } else {
            budget.max_lines
        };

        if lines.len() >= line_limit || (char_budget > 0 && next_chars > char_budget) {
            let remaining = violations.len() - idx;
            lines.push(format!("... (+{remaining} more violations truncated)"));
            return;
        }
        used_chars = next_chars;
        lines.push(formatted);
    }
}

fn format_violation_line(v: &QaViolation) -> String {
    let mut line = format!("- [{}]", v.kind);
    if let Some(vp) = &v.viewport {
        let _ = write!(line, " ({vp})");
    }
    let _ = write!(line, " {}: {}", v.selector, v.message);
    if let Some(r) = v.rect {
        let _ = write!(line, " [{},{},{}x{}]", r.x, r.y, r.width, r.height);
    }
    line
}

fn platform_str(platform: UiPlatform) -> &'static str {
    match platform {
        UiPlatform::Web => "web",
        UiPlatform::Mobile => "mobile",
        UiPlatform::Both => "both",
        UiPlatform::None => "none",
    }
}
