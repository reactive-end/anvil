//! Runs flows through Playwright and compares their screenshots with the stored baselines.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anvil_core::config::qa::WebQaConfig;
use anvil_core::proc_util::run_command_with_timeout;
use anyhow::{Context, Result, bail};

use super::diff::compare_files;
use super::eval::{RawRun, evaluate};
use super::plan::{build_flow_script, playwright_search_dirs};
use super::report::{BaselineStatus, FlowReport, RunSummary, ScreenshotResult, StepStatus};
use super::spec::{Flow, FlowEntry};

/// Folder (relative to the repository root) holding the stored baseline screenshots.
pub const BASELINE_DIR: &str = ".harness/qa/baseline";

/// Folder (relative to the repository root) that receives one sub-folder per run.
pub const RUNS_DIR: &str = ".harness/.state/qa";

const MIN_RUN_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RUN_TIMEOUT: Duration = Duration::from_secs(600);

/// Folder that receives one sub-folder per run.
pub fn runs_dir(root: &Path) -> PathBuf {
    join_rel(root, RUNS_DIR)
}

fn join_rel(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |path, part| path.join(part))
}

/// A sortable id for a run, such as `20261127-143015`.
pub fn new_run_id() -> String {
    chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string()
}

/// Where the baseline of screenshot `name` of flow `id` lives.
pub fn baseline_path(root: &Path, id: &str, name: &str) -> PathBuf {
    join_rel(root, BASELINE_DIR).join(id).join(format!("{name}.png"))
}

/// Runs each flow in turn; the screenshots and scripts of the run go under `.harness/.state/qa/<run id>/`.
pub fn run_flows(root: &Path, web: &WebQaConfig, entries: &[&FlowEntry]) -> RunSummary {
    let root = &std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
    let run_id = new_run_id();
    let run_dir = runs_dir(root).join(&run_id);
    let flows = entries
        .iter()
        .map(|entry| match &entry.flow {
            Ok(flow) => run_flow(root, web, &entry.id, flow, &run_dir),
            Err(err) => FlowReport::errored(&entry.id, &entry.id, format!("invalid flow: {err}")),
        })
        .collect();
    RunSummary::new(run_id, run_dir, flows)
}

fn run_flow(root: &Path, web: &WebQaConfig, id: &str, flow: &Flow, run_dir: &Path) -> FlowReport {
    let started = Instant::now();
    let out_dir = run_dir.join(id);
    let outcome = execute(root, web, id, flow, &out_dir);
    let mut report =
        outcome.unwrap_or_else(|err| FlowReport::errored(id, &flow.name, format!("{err:#}")));
    report.id = id.to_owned();
    report.name.clone_from(&flow.name);
    report.viewport = flow.viewport.dimensions().0.to_owned();
    report.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    report.finalize();
    report
}

fn execute(
    root: &Path,
    web: &WebQaConfig,
    id: &str,
    flow: &Flow,
    out_dir: &Path,
) -> Result<FlowReport> {
    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create {}", out_dir.display()))?;
    let hint = web.start_cwd.as_deref().map(|dir| root.join(dir));
    let dirs = playwright_search_dirs(root, hint.as_deref());
    let script = build_flow_script(flow, web.url.as_deref(), out_dir, &dirs)?;
    let script_path = out_dir.join("flow.mjs");
    fs::write(&script_path, script)
        .with_context(|| format!("failed to write {}", script_path.display()))?;

    let steps = u64::try_from(flow.steps.len()).unwrap_or(u64::MAX);
    let budget = Duration::from_millis(flow.step_timeout_ms().saturating_mul(steps))
        + Duration::from_secs(45);
    let mut command = Command::new("node");
    command.arg(&script_path).current_dir(root);
    let timed =
        run_command_with_timeout(&mut command, budget.clamp(MIN_RUN_TIMEOUT, MAX_RUN_TIMEOUT))
            .context("failed to invoke `node` (is Node.js installed?)")?;
    if timed.timed_out {
        bail!("flow runner timed out after {}s", budget.as_secs());
    }
    if timed.status_code != 0 {
        let stderr = String::from_utf8_lossy(&timed.stderr);
        let head: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).take(6).collect();
        bail!("flow runner failed: {}", head.join(" / "));
    }
    let stdout = String::from_utf8_lossy(&timed.stdout);
    let json = stdout.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or_default();
    let raw: RawRun =
        serde_json::from_str(json).context("failed to parse the flow runner output")?;
    build_report(root, web, id, flow, out_dir, &raw)
}

fn build_report(
    root: &Path,
    web: &WebQaConfig,
    id: &str,
    flow: &Flow,
    out_dir: &Path,
    raw: &RawRun,
) -> Result<FlowReport> {
    let evaluation = evaluate(flow, raw);
    let mut screenshots = Vec::new();
    for shot in &raw.screenshots {
        let baseline = baseline_path(root, id, &shot.name);
        let status = if baseline.is_file() {
            let diff_out = out_dir.join(format!("{}.diff.png", shot.name));
            compare_files(&baseline, &shot.path, web.threshold(), &diff_out)?
        } else {
            BaselineStatus::NoBaseline
        };
        screenshots.push(ScreenshotResult {
            name: shot.name.clone(),
            path: shot.path.clone(),
            baseline: status,
        });
    }
    let mut report = FlowReport::empty(id, &flow.name);
    report.steps = evaluation.steps;
    report.network.clone_from(&raw.network);
    report.console_errors = evaluation.console_errors;
    report.expected_console_errors = evaluation.expected_console_errors;
    report.page_errors.clone_from(&raw.page_errors);
    report.screenshots = screenshots;
    report.failure_screenshot.clone_from(&raw.failure_screenshot);
    Ok(report)
}

/// `true` when the flow itself worked (steps passed, no errors), so its screenshots may become baselines.
pub fn can_store_baselines(report: &FlowReport) -> bool {
    report.error.is_none()
        && report.console_errors.is_empty()
        && report.page_errors.is_empty()
        && report.steps.iter().all(|s| s.status == StepStatus::Passed)
}

/// Copies the screenshots of `report` to `.harness/qa/baseline/<flow>/<name>.png`.
pub fn store_baselines(root: &Path, report: &FlowReport) -> Result<Vec<PathBuf>> {
    let mut stored = Vec::new();
    for shot in &report.screenshots {
        let target = baseline_path(root, &report.id, &shot.name);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::copy(&shot.path, &target)
            .with_context(|| format!("failed to store baseline {}", target.display()))?;
        stored.push(target);
    }
    Ok(stored)
}
