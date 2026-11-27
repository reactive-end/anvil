//! Judges what the browser observed against the expectations of a flow.

use std::path::PathBuf;

use serde::Deserialize;

use super::report::{NetworkEntry, StepResult, StepStatus};
use super::spec::{Flow, NoRequestExpect, RequestExpect, Step};

/// What the generated Playwright script observed, before expectations are judged.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawRun {
    #[serde(default)]
    pub steps: Vec<RawStep>,
    #[serde(default)]
    pub network: Vec<NetworkEntry>,
    #[serde(default)]
    pub console: Vec<RawConsole>,
    #[serde(default)]
    pub page_errors: Vec<String>,
    #[serde(default)]
    pub screenshots: Vec<RawShot>,
    #[serde(default)]
    pub failure_screenshot: Option<PathBuf>,
}

/// Observation of one executed step.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawStep {
    /// 0-based position in the flow.
    pub index: usize,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// Requests with a `seq` below this value happened before the step's observation window.
    #[serde(default)]
    pub since: u64,
    /// Requests with a `seq` above this value started after the step finished.
    #[serde(default)]
    pub until: Option<u64>,
    #[serde(default)]
    pub found: Option<bool>,
    #[serde(default)]
    pub visible: Option<bool>,
    #[serde(default)]
    pub count: Option<usize>,
}

/// A console message of type `error`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawConsole {
    pub text: String,
    /// Resource the message refers to (set for failed resource loads).
    #[serde(default)]
    pub url: Option<String>,
}

/// A screenshot file written by the script.
#[derive(Debug, Clone, Deserialize)]
pub struct RawShot {
    pub name: String,
    pub path: PathBuf,
}

/// Step results plus the console errors split into real and expected ones.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Evaluation {
    pub steps: Vec<StepResult>,
    pub console_errors: Vec<String>,
    pub expected_console_errors: Vec<String>,
}

/// Judges every step of `flow` against `raw`, in order.
pub fn evaluate(flow: &Flow, raw: &RawRun) -> Evaluation {
    let steps: Vec<StepResult> =
        flow.steps.iter().enumerate().map(|(idx, step)| judge_step(idx, step, raw)).collect();
    let (expected, real): (Vec<&RawConsole>, Vec<&RawConsole>) =
        raw.console.iter().partition(|entry| is_expected_failure(entry, flow, &steps));
    Evaluation {
        steps,
        console_errors: real.iter().map(|e| e.text.clone()).collect(),
        expected_console_errors: expected.iter().map(|e| e.text.clone()).collect(),
    }
}

fn judge_step(idx: usize, step: &Step, raw: &RawRun) -> StepResult {
    let mut result = StepResult {
        index: idx + 1,
        step: step.describe(),
        status: StepStatus::Passed,
        reason: None,
        matched_requests: Vec::new(),
    };
    let Some(seen) = raw.steps.iter().find(|s| s.index == idx) else {
        result.status = StepStatus::Skipped;
        return result;
    };
    if !seen.ok {
        return fail(result, seen.error.clone().unwrap_or_else(|| "step failed".to_owned()));
    }
    let until = seen.until.unwrap_or(u64::MAX);
    let window: Vec<&NetworkEntry> =
        raw.network.iter().filter(|r| r.seq >= seen.since && r.seq <= until).collect();
    let verdict = match step {
        Step::ExpectRequest(expect) => judge_request(expect, &window, &mut result),
        Step::ExpectNoRequest(expect) => judge_no_request(expect, &window),
        Step::ExpectText(text) => {
            ensure(seen.found == Some(true), || format!("text `{text}` was not found on the page"))
        }
        Step::ExpectVisible(sel) => {
            ensure(seen.visible == Some(true), || format!("`{sel}` is not visible"))
        }
        Step::ExpectCount(expect) => ensure(seen.count == Some(expect.count), || {
            let found = seen.count.map_or_else(|| "unknown".to_owned(), |n| n.to_string());
            format!("`{}` matched {found} element(s), expected {}", expect.selector, expect.count)
        }),
        _ => Ok(()),
    };
    match verdict {
        Ok(()) => result,
        Err(reason) => fail(result, reason),
    }
}

fn fail(mut result: StepResult, reason: String) -> StepResult {
    result.status = StepStatus::Failed;
    result.reason = Some(reason);
    result
}

fn ensure(ok: bool, reason: impl FnOnce() -> String) -> Result<(), String> {
    if ok { Ok(()) } else { Err(reason()) }
}

fn request_matches(expect: &RequestExpect, entry: &NetworkEntry) -> bool {
    expect.method.as_deref().is_none_or(|m| m.eq_ignore_ascii_case(&entry.method))
        && entry.url.contains(&expect.url_contains)
}

fn judge_request(
    expect: &RequestExpect,
    window: &[&NetworkEntry],
    result: &mut StepResult,
) -> Result<(), String> {
    let candidates: Vec<&NetworkEntry> =
        window.iter().copied().filter(|entry| request_matches(expect, entry)).collect();
    let what = expect.describe().replacen("expect_request ", "", 1);
    if candidates.is_empty() {
        return Err(format!("no request matching {what} was made{}", seen_hint(window)));
    }
    let Some(wanted) = expect.status else {
        result.matched_requests = candidates.into_iter().cloned().collect();
        return Ok(());
    };
    let right: Vec<&NetworkEntry> =
        candidates.iter().copied().filter(|entry| entry.status == Some(wanted)).collect();
    if right.is_empty() {
        let got: Vec<String> = candidates.iter().map(|entry| entry.brief()).collect();
        result.matched_requests = candidates.into_iter().cloned().collect();
        return Err(format!("expected status {wanted} but saw {}", got.join("; ")));
    }
    result.matched_requests = right.into_iter().cloned().collect();
    Ok(())
}

fn judge_no_request(expect: &NoRequestExpect, window: &[&NetworkEntry]) -> Result<(), String> {
    match window.iter().find(|entry| entry.url.contains(&expect.url_contains)) {
        Some(entry) => Err(format!("unexpected request {}", entry.brief())),
        None => Ok(()),
    }
}

/// Up to five requests of the window, so a missing request can be compared with what happened.
fn seen_hint(window: &[&NetworkEntry]) -> String {
    let api: Vec<String> = window
        .iter()
        .filter(|entry| matches!(entry.resource_type.as_str(), "fetch" | "xhr" | "document"))
        .take(5)
        .map(|entry| entry.brief())
        .collect();
    if api.is_empty() {
        " (no fetch/xhr requests in that window)".to_owned()
    } else {
        format!(" (saw: {})", api.join("; "))
    }
}

/// A "Failed to load resource" message caused by a request a passing `expect_request` step expects to fail.
fn is_expected_failure(entry: &RawConsole, flow: &Flow, steps: &[StepResult]) -> bool {
    if !entry.text.starts_with("Failed to load resource") {
        return false;
    }
    flow.steps.iter().zip(steps).any(|(step, result)| {
        let Step::ExpectRequest(expect) = step else { return false };
        let Some(status) = expect.status.filter(|s| *s >= 400) else { return false };
        result.status == StepStatus::Passed
            && entry.text.contains(&format!("status of {status}"))
            && entry.url.as_deref().is_some_and(|url| url.contains(&expect.url_contains))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(seq: u64, method: &str, url: &str, status: Option<u16>) -> NetworkEntry {
        NetworkEntry {
            seq,
            method: method.to_owned(),
            url: url.to_owned(),
            status,
            resource_type: "fetch".to_owned(),
            duration_ms: Some(5),
            failure: None,
        }
    }

    fn flow(steps: &str) -> Flow {
        Flow::parse(&format!(r#"{{"name":"t","steps":{steps}}}"#)).expect("flow")
    }

    fn step_ok(index: usize) -> RawStep {
        RawStep { index, ok: true, ..RawStep::default() }
    }

    fn judged(flow: &Flow, raw: &RawRun) -> Vec<(StepStatus, Option<String>)> {
        evaluate(flow, raw).steps.into_iter().map(|s| (s.status, s.reason)).collect()
    }

    #[test]
    fn request_matched_with_status_passes_and_lists_the_match() {
        let f =
            flow(r#"[{"expect_request":{"method":"post","url_contains":"/orders","status":201}}]"#);
        let raw = RawRun {
            steps: vec![step_ok(0)],
            network: vec![
                net(1, "GET", "http://x/orders", Some(200)),
                net(2, "POST", "http://x/orders", Some(201)),
            ],
            ..RawRun::default()
        };
        let out = evaluate(&f, &raw);
        assert_eq!(out.steps[0].status, StepStatus::Passed);
        assert_eq!(out.steps[0].matched_requests.len(), 1);
        assert_eq!(out.steps[0].matched_requests[0].seq, 2);
    }

    #[test]
    fn missing_request_fails_and_hints_what_was_seen() {
        let f = flow(r#"[{"expect_request":{"method":"POST","url_contains":"/orders"}}]"#);
        let raw = RawRun {
            steps: vec![step_ok(0)],
            network: vec![net(1, "GET", "http://x/orders", Some(200))],
            ..RawRun::default()
        };
        let verdict = judged(&f, &raw);
        assert_eq!(verdict[0].0, StepStatus::Failed);
        let reason = verdict[0].1.clone().unwrap_or_default();
        assert!(reason.contains("no request matching POST *"), "{reason}");
        assert!(reason.contains("GET http://x/orders -> 200"), "{reason}");
    }

    #[test]
    fn wrong_status_fails_naming_both_statuses() {
        let f =
            flow(r#"[{"expect_request":{"method":"POST","url_contains":"/orders","status":200}}]"#);
        let raw = RawRun {
            steps: vec![step_ok(0)],
            network: vec![net(1, "POST", "http://x/orders", Some(201))],
            ..RawRun::default()
        };
        let reason = judged(&f, &raw)[0].1.clone().unwrap_or_default();
        assert!(reason.contains("expected status 200"), "{reason}");
        assert!(reason.contains("POST http://x/orders -> 201"), "{reason}");
    }

    #[test]
    fn window_hides_requests_that_started_before_the_step_window() {
        let f = flow(
            r#"[{"expect_no_request":{"url_contains":"/customers"}},{"expect_request":{"url_contains":"/orders"}}]"#,
        );
        let early = net(1, "GET", "http://x/customers", Some(200));
        let late = net(2, "GET", "http://x/orders", Some(200));
        let raw = RawRun {
            steps: vec![RawStep { since: 2, ..step_ok(0) }, RawStep { since: 2, ..step_ok(1) }],
            network: vec![early.clone(), late],
            ..RawRun::default()
        };
        assert!(judged(&f, &raw).iter().all(|(s, _)| *s == StepStatus::Passed));
        let raw_all = RawRun { steps: vec![step_ok(0)], network: vec![early], ..RawRun::default() };
        let reason = judged(&f, &raw_all)[0].1.clone().unwrap_or_default();
        assert!(reason.contains("unexpected request GET http://x/customers -> 200"), "{reason}");
    }

    #[test]
    fn requests_after_the_step_finished_are_outside_its_window() {
        let f = flow(r#"[{"expect_no_request":{"url_contains":"/orders"}}]"#);
        let raw = RawRun {
            steps: vec![RawStep { until: Some(1), ..step_ok(0) }],
            network: vec![
                net(1, "GET", "http://x/customers", Some(200)),
                net(2, "POST", "http://x/orders", Some(201)),
            ],
            ..RawRun::default()
        };
        assert_eq!(judged(&f, &raw)[0].0, StepStatus::Passed);
    }

    #[test]
    fn text_visible_and_count_use_the_observations() {
        let f = flow(
            r##"[{"expect_text":"Widget"},{"expect_visible":"#a"},{"expect_count":{"selector":"tr","count":3}},{"expect_count":{"selector":"tr","count":3}}]"##,
        );
        let raw = RawRun {
            steps: vec![
                RawStep { found: Some(true), ..step_ok(0) },
                RawStep { visible: Some(false), ..step_ok(1) },
                RawStep { count: Some(3), ..step_ok(2) },
                RawStep { count: Some(2), ..step_ok(3) },
            ],
            ..RawRun::default()
        };
        let verdict = judged(&f, &raw);
        assert_eq!(verdict[0].0, StepStatus::Passed);
        assert_eq!(verdict[1].1.as_deref(), Some("`#a` is not visible"));
        assert_eq!(verdict[2].0, StepStatus::Passed);
        assert_eq!(verdict[3].1.as_deref(), Some("`tr` matched 2 element(s), expected 3"));
    }

    #[test]
    fn failed_action_is_reported_and_later_steps_are_skipped() {
        let f = flow(r##"[{"click":"#a"},{"click":"#b"}]"##);
        let raw = RawRun {
            steps: vec![RawStep {
                error: Some("timeout waiting for `#a`".to_owned()),
                ..RawStep::default()
            }],
            ..RawRun::default()
        };
        let verdict = judged(&f, &raw);
        assert_eq!(verdict[0], (StepStatus::Failed, Some("timeout waiting for `#a`".to_owned())));
        assert_eq!(verdict[1].0, StepStatus::Skipped);
    }

    #[test]
    fn console_error_of_an_expected_failure_is_set_aside() {
        let f =
            flow(r#"[{"expect_request":{"method":"POST","url_contains":"/orders","status":400}}]"#);
        let failed_load = RawConsole {
            text:
                "Failed to load resource: the server responded with a status of 400 (Bad Request)"
                    .to_owned(),
            url: Some("http://x/orders".to_owned()),
        };
        let other = RawConsole { text: "boom".to_owned(), url: None };
        let raw = RawRun {
            steps: vec![step_ok(0)],
            network: vec![net(1, "POST", "http://x/orders", Some(400))],
            console: vec![failed_load, other],
            ..RawRun::default()
        };
        let out = evaluate(&f, &raw);
        assert_eq!(out.console_errors, vec!["boom".to_owned()]);
        assert_eq!(out.expected_console_errors.len(), 1);
    }
}
