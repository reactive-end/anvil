//! What a scenario run produced, in the shape written to `report.json`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::spec::Step;

/// Outcome of one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Passed,
    Failed,
    /// Not run because an earlier step of the scenario failed.
    Skipped,
}

/// One step with what happened: the call made, why it failed, and the values it captured.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepReport {
    /// 1-based position in the scenario.
    pub index: usize,
    pub name: String,
    /// `http` or `db`.
    pub kind: String,
    pub status: StepStatus,
    /// The call and its result in a few words, such as `POST /orders -> 201`.
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What the API or database actually returned (truncated), when the step failed on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<Value>,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub captured: BTreeMap<String, Value>,
}

impl StepReport {
    /// A passed step that has not run yet; the runner fills in the rest.
    pub fn start(index: usize, step: &Step) -> Self {
        Self {
            index,
            name: step.label(),
            kind: step.kind().to_owned(),
            status: StepStatus::Passed,
            summary: step.label(),
            reason: None,
            actual: None,
            duration_ms: 0,
            captured: BTreeMap::new(),
        }
    }

    /// A step that never ran because an earlier one failed.
    pub fn skipped(index: usize, step: &Step) -> Self {
        let mut report = Self::start(index, step);
        report.status = StepStatus::Skipped;
        report.reason = Some("not run because an earlier step failed".to_owned());
        report
    }

    /// Marks the step failed with `reason` and, when known, what was actually returned.
    pub fn fail(&mut self, reason: String, actual: Option<Value>) {
        self.status = StepStatus::Failed;
        self.reason = Some(reason);
        self.actual = actual;
    }
}

/// One scenario file's run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioReport {
    /// File stem.
    pub id: String,
    pub name: String,
    pub passed: bool,
    pub duration_ms: u64,
    pub steps: Vec<StepReport>,
    /// Set when the scenario could not run at all (invalid file, bad variables).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ScenarioReport {
    /// A scenario that could not run.
    pub fn errored(id: &str, name: &str, error: String) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            passed: false,
            duration_ms: 0,
            steps: Vec::new(),
            error: Some(error),
        }
    }

    /// Sets `passed` from the steps and the error.
    pub fn finalize(&mut self) {
        self.passed =
            self.error.is_none() && self.steps.iter().all(|s| s.status == StepStatus::Passed);
    }
}

/// Every scenario of one run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: String,
    pub run_dir: PathBuf,
    pub passed: bool,
    pub scenarios: Vec<ScenarioReport>,
    /// Where `report.json` was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_path: Option<PathBuf>,
    /// Why `report.json` could not be written; the results above are still valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_error: Option<String>,
}

impl RunSummary {
    pub fn new(run_id: String, run_dir: PathBuf, scenarios: Vec<ScenarioReport>) -> Self {
        let passed = scenarios.iter().all(|s| s.passed);
        Self { run_id, run_dir, passed, scenarios, report_path: None, report_error: None }
    }
}
