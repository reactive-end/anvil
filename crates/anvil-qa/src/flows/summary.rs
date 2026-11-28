//! Compact text rendering of flow results for terminals and `anvil check`.

use super::report::{BaselineStatus, FlowReport, RunSummary, StepStatus};

/// One block per flow: a status line, then only what needs attention.
pub fn format_run(summary: &RunSummary) -> String {
    let mut lines: Vec<String> = summary.flows.iter().flat_map(flow_lines).collect();
    let failed = summary.flows.iter().filter(|f| !f.passed).count();
    lines.push(format!(
        "{} of {} flow(s) passed; artifacts in {}",
        summary.flows.len() - failed,
        summary.flows.len(),
        summary.run_dir.display()
    ));
    lines.join("\n")
}

/// Lines describing one flow (`[PASS]`/`[FAIL]` header plus findings).
pub fn flow_lines(report: &FlowReport) -> Vec<String> {
    let mark = if report.passed { "PASS" } else { "FAIL" };
    let steps_ok = report.steps.iter().filter(|s| s.status == StepStatus::Passed).count();
    let matched: usize = report
        .steps
        .iter()
        .filter(|s| s.status == StepStatus::Passed)
        .map(|s| s.matched_requests.len())
        .sum();
    let same = report
        .screenshots
        .iter()
        .filter(|s| matches!(s.baseline, BaselineStatus::Match { .. }))
        .count();
    let baselines = if same > 0 {
        format!(", {same} screenshot(s) match their baseline")
    } else {
        String::new()
    };
    let mut lines = vec![format!(
        "[{mark}] {} ({}, {:.1}s): {steps_ok}/{} steps, {} request(s) logged, {matched} matched by expectations{baselines}",
        report.id,
        report.viewport,
        report.duration_ms as f64 / 1000.0,
        report.steps.len(),
        report.network.len(),
    )];
    if let Some(error) = &report.error {
        lines.push(format!("  error: {error}"));
    }
    for step in report.steps.iter().filter(|s| s.status != StepStatus::Passed) {
        let reason = step.reason.as_deref().unwrap_or("not run because an earlier step failed");
        lines.push(format!("  step {} {}: {reason}", step.index, step.step));
    }
    for entry in report.unexpected_errors() {
        lines.push(format!("  unexpected response: {}", entry.brief()));
    }
    for text in &report.console_errors {
        lines.push(format!("  console error: {text}"));
    }
    for text in &report.page_errors {
        lines.push(format!("  page error: {text}"));
    }
    lines.extend(report.screenshots.iter().filter_map(|s| shot_line(&s.name, &s.baseline)));
    if let Some(path) = &report.failure_screenshot {
        lines.push(format!("  failure screenshot: {}", path.display()));
    }
    lines
}

fn shot_line(name: &str, status: &BaselineStatus) -> Option<String> {
    match status {
        BaselineStatus::Match { .. } => None,
        BaselineStatus::NoBaseline => Some(format!(
            "  note: screenshot `{name}` has no baseline yet (`anvil qa baseline` stores it)"
        )),
        BaselineStatus::Diff { ratio, threshold, diff_path } => Some(format!(
            "  visual diff: `{name}` changed {:.2}% of pixels (threshold {:.2}%); diff image {}",
            ratio * 100.0,
            threshold * 100.0,
            diff_path.display()
        )),
        BaselineStatus::SizeMismatch { baseline, actual } => Some(format!(
            "  visual diff: `{name}` size changed from {}x{} (baseline) to {}x{}",
            baseline.0, baseline.1, actual.0, actual.1
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::flows::report::{NetworkEntry, ScreenshotResult, StepResult};

    fn step(index: usize, status: StepStatus, reason: Option<&str>) -> StepResult {
        StepResult {
            index,
            step: format!("step{index}"),
            status,
            reason: reason.map(str::to_owned),
            matched_requests: Vec::new(),
        }
    }

    #[test]
    fn a_failing_flow_lists_each_finding_once() {
        let mut report = FlowReport::empty("create-order", "create order");
        report.viewport = "desktop".to_owned();
        report.steps = vec![
            step(1, StepStatus::Passed, None),
            step(2, StepStatus::Failed, Some("expected status 201 but saw POST /o -> 500")),
            step(3, StepStatus::Skipped, None),
        ];
        report.network = vec![NetworkEntry {
            seq: 1,
            method: "POST".to_owned(),
            url: "http://x/o".to_owned(),
            status: Some(500),
            resource_type: "fetch".to_owned(),
            duration_ms: Some(3),
            failure: None,
        }];
        report.console_errors = vec!["boom".to_owned()];
        report.screenshots = vec![ScreenshotResult {
            name: "after".to_owned(),
            path: PathBuf::from("a.png"),
            baseline: BaselineStatus::Diff {
                ratio: 0.032,
                threshold: 0.01,
                diff_path: PathBuf::from("a.diff.png"),
            },
        }];
        report.finalize();
        let text = flow_lines(&report).join("\n");
        assert!(text.starts_with("[FAIL] create-order (desktop"), "{text}");
        assert!(text.contains("step 2 step2: expected status 201"), "{text}");
        assert!(text.contains("step 3 step3: not run"), "{text}");
        assert!(text.contains("unexpected response: POST http://x/o -> 500"), "{text}");
        assert!(text.contains("console error: boom"), "{text}");
        assert!(text.contains("changed 3.20% of pixels (threshold 1.00%)"), "{text}");
    }

    #[test]
    fn a_passing_flow_is_one_line_plus_baseline_notes() {
        let mut report = FlowReport::empty("f", "f");
        report.steps = vec![step(1, StepStatus::Passed, None)];
        report.screenshots = vec![ScreenshotResult {
            name: "s".to_owned(),
            path: PathBuf::from("s.png"),
            baseline: BaselineStatus::NoBaseline,
        }];
        report.finalize();
        let lines = flow_lines(&report);
        assert!(report.passed);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("no baseline yet"));
    }
}
