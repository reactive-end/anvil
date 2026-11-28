//! Compact text rendering of scenario results for terminals and `anvil check`.

use super::report::{RunSummary, ScenarioReport, StepStatus};

/// One block per scenario: a status line, then only what needs attention.
pub fn format_run(summary: &RunSummary) -> String {
    let mut lines: Vec<String> = summary.scenarios.iter().flat_map(scenario_lines).collect();
    let failed = summary.scenarios.iter().filter(|s| !s.passed).count();
    lines.push(format!(
        "{} of {} scenario(s) passed",
        summary.scenarios.len() - failed,
        summary.scenarios.len(),
    ));
    match (&summary.report_path, &summary.report_error) {
        (Some(path), _) => lines.push(format!("report: {}", path.display())),
        (None, Some(err)) => lines.push(format!("report could not be written: {err}")),
        (None, None) => {}
    }
    lines.join("\n")
}

/// Lines describing one scenario (`[PASS]`/`[FAIL]` header plus the failing step).
pub fn scenario_lines(report: &ScenarioReport) -> Vec<String> {
    let mark = if report.passed { "PASS" } else { "FAIL" };
    let passed = report.steps.iter().filter(|s| s.status == StepStatus::Passed).count();
    let mut lines = vec![format!(
        "[{mark}] {} ({:.1}s): {passed}/{} steps",
        report.id,
        report.duration_ms as f64 / 1000.0,
        report.steps.len()
    )];
    if let Some(error) = &report.error {
        lines.push(format!("  error: {error}"));
    }
    for step in report.steps.iter().filter(|s| s.status == StepStatus::Failed) {
        lines.push(format!(
            "  step {} {}: {}",
            step.index,
            step.summary,
            step.reason.as_deref().unwrap_or("failed")
        ));
        if let Some(actual) = &step.actual {
            lines.push(format!("    actual: {actual}"));
        }
    }
    let skipped = report.steps.iter().filter(|s| s.status == StepStatus::Skipped).count();
    if skipped > 0 {
        lines.push(format!("  {skipped} later step(s) not run"));
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::api::report::StepReport;

    fn step(index: usize, status: StepStatus, reason: Option<&str>) -> StepReport {
        StepReport {
            index,
            name: "n".to_owned(),
            kind: "http".to_owned(),
            status,
            summary: format!("POST /orders -> {index}"),
            reason: reason.map(str::to_owned),
            actual: reason.map(|_| json!({ "message": "no" })),
            duration_ms: 1,
            captured: Default::default(),
        }
    }

    #[test]
    fn failing_scenarios_show_the_step_the_reason_and_the_actual_value() {
        let mut report = ScenarioReport {
            id: "create".to_owned(),
            name: "create".to_owned(),
            passed: false,
            duration_ms: 1200,
            steps: vec![
                step(1, StepStatus::Passed, None),
                step(2, StepStatus::Failed, Some("expected status 201 but got 404")),
                step(3, StepStatus::Skipped, None),
            ],
            error: None,
        };
        report.finalize();
        let summary = RunSummary::new("r".to_owned(), PathBuf::from("run"), vec![report]);
        let text = format_run(&summary);
        assert!(text.contains("[FAIL] create (1.2s): 1/3 steps"), "{text}");
        assert!(
            text.contains("step 2 POST /orders -> 2: expected status 201 but got 404"),
            "{text}"
        );
        assert!(text.contains("actual: {\"message\":\"no\"}"), "{text}");
        assert!(text.contains("1 later step(s) not run"), "{text}");
        assert!(text.contains("0 of 1 scenario(s) passed"), "{text}");
    }
}
