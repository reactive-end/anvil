//! Structured results of running flows.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One request the page made, as seen by the browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkEntry {
    /// Order in which the request started (1-based).
    pub seq: u64,
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub resource_type: String,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// Network-level failure (`net::ERR_...`) when no response arrived.
    #[serde(default)]
    pub failure: Option<String>,
}

impl NetworkEntry {
    /// `METHOD url -> status`, or the failure text.
    pub fn brief(&self) -> String {
        let outcome = match (self.status, &self.failure) {
            (Some(status), _) => status.to_string(),
            (None, Some(failure)) => failure.clone(),
            (None, None) => "no response".to_owned(),
        };
        format!("{} {} -> {outcome}", self.method, self.url)
    }

    /// `true` for a 4xx/5xx answer or a failed request.
    pub fn is_error(&self) -> bool {
        self.status.is_some_and(|s| s >= 400) || (self.status.is_none() && self.failure.is_some())
    }
}

/// Outcome of one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Passed,
    Failed,
    /// Not run because an earlier step failed.
    Skipped,
}

/// Result of one step, with the requests an `expect_request` step matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepResult {
    /// Position in the flow (1-based).
    pub index: usize,
    pub step: String,
    pub status: StepStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_requests: Vec<NetworkEntry>,
}

/// How a screenshot compares with its stored baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum BaselineStatus {
    /// No baseline stored yet (`anvil qa baseline` stores one).
    NoBaseline,
    /// Identical, or changed by no more than the threshold.
    Match {
        ratio: f64,
    },
    /// Changed pixels exceed the threshold.
    Diff {
        ratio: f64,
        threshold: f64,
        diff_path: PathBuf,
    },
    SizeMismatch {
        baseline: (u32, u32),
        actual: (u32, u32),
    },
}

impl BaselineStatus {
    /// `true` when the screenshot must fail the run.
    pub fn failed(&self) -> bool {
        matches!(self, Self::Diff { .. } | Self::SizeMismatch { .. })
    }
}

/// A screenshot taken by a `screenshot` step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenshotResult {
    pub name: String,
    pub path: PathBuf,
    pub baseline: BaselineStatus,
}

/// Everything one flow run produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowReport {
    /// File stem of the flow.
    pub id: String,
    pub name: String,
    pub viewport: String,
    pub passed: bool,
    pub duration_ms: u64,
    pub steps: Vec<StepResult>,
    pub network: Vec<NetworkEntry>,
    pub console_errors: Vec<String>,
    /// Console errors caused by a request the flow expects to fail; they do not fail the run.
    pub expected_console_errors: Vec<String>,
    pub page_errors: Vec<String>,
    pub screenshots: Vec<ScreenshotResult>,
    /// Screenshot of the page at the moment a step failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_screenshot: Option<PathBuf>,
    /// Runner-level failure (node missing, Playwright missing, flow file invalid).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl FlowReport {
    /// A report for a flow that could not run at all.
    pub fn errored(id: &str, name: &str, error: String) -> Self {
        Self { error: Some(error), ..Self::empty(id, name) }
    }

    /// A report with no observations yet.
    pub fn empty(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            viewport: String::new(),
            passed: false,
            duration_ms: 0,
            steps: Vec::new(),
            network: Vec::new(),
            console_errors: Vec::new(),
            expected_console_errors: Vec::new(),
            page_errors: Vec::new(),
            screenshots: Vec::new(),
            failure_screenshot: None,
            error: None,
        }
    }

    /// Recomputes `passed` from steps, errors, and screenshot comparisons.
    pub fn finalize(&mut self) {
        self.passed = self.error.is_none()
            && self.console_errors.is_empty()
            && self.page_errors.is_empty()
            && self.steps.iter().all(|s| s.status == StepStatus::Passed)
            && self.screenshots.iter().all(|s| !s.baseline.failed());
    }

    /// Requests that answered 4xx/5xx or failed, except those an `expect_request` step accounted for.
    pub fn unexpected_errors(&self) -> Vec<&NetworkEntry> {
        let accounted: Vec<u64> = self
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Passed)
            .flat_map(|s| s.matched_requests.iter().map(|r| r.seq))
            .collect();
        self.network.iter().filter(|r| r.is_error() && !accounted.contains(&r.seq)).collect()
    }
}

/// Result of running a set of flows in one run directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: String,
    pub run_dir: PathBuf,
    pub passed: bool,
    pub flows: Vec<FlowReport>,
}

impl RunSummary {
    /// Builds a summary whose `passed` is `true` only when every flow passed.
    pub fn new(run_id: String, run_dir: PathBuf, flows: Vec<FlowReport>) -> Self {
        let passed = flows.iter().all(|f| f.passed);
        Self { run_id, run_dir, passed, flows }
    }
}
