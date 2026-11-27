//! Runs scenarios in order and records the run under `.harness/.state/qa/api/<run id>/`.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use anvil_core::config::qa::ApiQaConfig;
use anyhow::{Context, Result};

use super::db::DbClient;
use super::exec::{StepContext, run_step};
use super::interpolate::{self, RUN_VAR, Vars};
use super::report::{RunSummary, ScenarioReport, StepReport, StepStatus};
use super::spec::{Scenario, ScenarioEntry};
use crate::flows::run::runs_dir;

/// Folder (inside the QA runs folder) that receives one sub-folder per API run.
pub const API_RUNS_SUBDIR: &str = "api";

/// Name of the file that holds the full structured result of a run.
pub const REPORT_FILE: &str = "report.json";

static RUN_COUNTER: AtomicU32 = AtomicU32::new(0);

/// A sortable id for a run, such as `20261127-143015-482`.
pub fn new_run_id() -> String {
    chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f").to_string()
}

/// A value that differs for every scenario execution, so emails and names never collide.
fn new_run_token() -> String {
    let millis = chrono::Utc::now().timestamp_millis();
    format!("{millis}-{}", RUN_COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Runs each scenario in turn. A failing step stops its scenario, not the run.
pub fn run_scenarios(
    root: &Path,
    api: &ApiQaConfig,
    db: &dyn DbClient,
    entries: &[&ScenarioEntry],
) -> RunSummary {
    let run_id = new_run_id();
    let run_dir = runs_dir(root).join(API_RUNS_SUBDIR).join(&run_id);
    let ctx = StepContext { base_url: api.url.as_deref(), db };
    let scenarios = entries
        .iter()
        .map(|entry| match &entry.scenario {
            Ok(scenario) => run_scenario(&ctx, &entry.id, scenario),
            Err(err) => {
                ScenarioReport::errored(&entry.id, &entry.id, format!("invalid scenario: {err}"))
            }
        })
        .collect();
    let mut summary = RunSummary::new(run_id, run_dir, scenarios);
    match write_report(&summary) {
        Ok(path) => summary.report_path = Some(path),
        Err(err) => summary.report_error = Some(format!("{err:#}")),
    }
    summary
}

fn run_scenario(ctx: &StepContext<'_>, id: &str, scenario: &Scenario) -> ScenarioReport {
    let started = Instant::now();
    let mut report = match initial_vars(scenario) {
        Ok(mut vars) => execute_steps(ctx, id, scenario, &mut vars),
        Err(err) => ScenarioReport::errored(id, &scenario.name, format!("{err:#}")),
    };
    report.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    report.finalize();
    report
}

/// `$run` plus the file's `vars`, whose strings may use `{{$run}}`.
fn initial_vars(scenario: &Scenario) -> Result<Vars> {
    let mut vars = Vars::new();
    vars.insert(RUN_VAR.to_owned(), serde_json::Value::String(new_run_token()));
    for (name, value) in &scenario.vars {
        let filled =
            interpolate::value(value, &vars).with_context(|| format!("variable `{name}`"))?;
        vars.insert(name.clone(), filled);
    }
    Ok(vars)
}

fn execute_steps(
    ctx: &StepContext<'_>,
    id: &str,
    scenario: &Scenario,
    vars: &mut Vars,
) -> ScenarioReport {
    let mut steps: Vec<StepReport> = Vec::new();
    let mut failed = false;
    for (offset, step) in scenario.steps.iter().enumerate() {
        let index = offset + 1;
        let report = if failed {
            StepReport::skipped(index, step)
        } else {
            run_step(ctx, vars, index, step)
        };
        failed |= report.status == StepStatus::Failed;
        steps.push(report);
    }
    ScenarioReport {
        id: id.to_owned(),
        name: scenario.name.clone(),
        passed: false,
        duration_ms: 0,
        steps,
        error: None,
    }
}

fn write_report(summary: &RunSummary) -> Result<std::path::PathBuf> {
    fs::create_dir_all(&summary.run_dir)
        .with_context(|| format!("failed to create {}", summary.run_dir.display()))?;
    let path = summary.run_dir.join(REPORT_FILE);
    let json = serde_json::to_string_pretty(summary).context("failed to serialize the report")?;
    fs::write(&path, format!("{json}\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::api::spec::load_scenarios;
    use crate::api::testkit::{FakeDb, serve_with};

    fn entries(dir: &Path, scenarios: &[(&str, &str)]) -> Vec<ScenarioEntry> {
        for (name, body) in scenarios {
            fs::write(dir.join(format!("{name}.json")), body).expect("write");
        }
        load_scenarios(dir).expect("load")
    }

    fn api(url: &str) -> ApiQaConfig {
        ApiQaConfig { url: Some(url.to_owned()), ..ApiQaConfig::default() }
    }

    const CREATE_AND_PAY: &str = r#"{
        "name": "create and pay",
        "vars": { "email": "qa-{{$run}}@example.com" },
        "steps": [
            { "http": { "method": "POST", "path": "/orders", "json": { "email": "{{email}}", "quantity": 2 } },
              "expect": { "status": 201, "json": { "/quantity": 2 } }, "capture": { "orderId": "/id" } },
            { "name": "order stored", "db": { "tool": "order_by_id", "params": { "id": "{{orderId}}" } },
              "expect": { "rows": 1, "row": { "status": "created", "quantity": 2 } } },
            { "http": { "method": "POST", "path": "/orders/{{orderId}}/pay" }, "expect": { "status": 200 } },
            { "db": { "sql": "SELECT status FROM orders WHERE id = '{{orderId}}'" },
              "expect": { "row": { "status": "paid" } }, "capture": { "status": "/0/status" } }
        ]
    }"#;

    fn orders_server() -> crate::api::testkit::TestServer {
        serve_with(|request| {
            if request.starts_with("POST /orders HTTP") {
                ("201 Created".to_owned(), r#"{"id":"o-7","quantity":2}"#.to_owned())
            } else if request.starts_with("POST /orders/o-7/pay") {
                ("200 OK".to_owned(), r#"{"id":"o-7","status":"paid"}"#.to_owned())
            } else {
                ("404 Not Found".to_owned(), r#"{"message":"no route"}"#.to_owned())
            }
        })
    }

    #[test]
    fn a_scenario_calls_the_api_checks_the_database_and_writes_a_report() {
        let dir = tempfile::tempdir().expect("tmp");
        let server = orders_server();
        let db = FakeDb::new()
            .then_rows(vec![json!({ "status": "created", "quantity": 2 })])
            .then_rows(vec![json!({ "status": "paid" })]);
        let found = entries(dir.path(), &[("create-and-pay", CREATE_AND_PAY)]);
        let refs: Vec<&ScenarioEntry> = found.iter().collect();
        let summary = run_scenarios(dir.path(), &api(&server.url), &db, &refs);

        let scenario = &summary.scenarios[0];
        assert!(summary.passed, "{:?}", scenario.steps);
        assert_eq!(scenario.steps.len(), 4);
        assert_eq!(scenario.steps[0].captured["orderId"], json!("o-7"));
        let calls = db.calls();
        assert_eq!(calls[0], ("order_by_id".to_owned(), json!({ "id": "o-7" })));
        assert_eq!(calls[1].0, "anvil_select");
        assert_eq!(calls[1].1["sql"], json!("SELECT status FROM orders WHERE id = 'o-7'"));
        let sent = server.requests();
        assert!(sent[0].contains("qa-"), "{}", sent[0]);
        let path = summary.report_path.expect("report written");
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).expect("read")).expect("json");
        assert_eq!(saved["passed"], json!(true));
        assert_eq!(saved["scenarios"][0]["steps"][3]["captured"]["status"], json!("paid"));
    }

    #[test]
    fn the_first_failing_step_stops_its_scenario_and_the_run_goes_on() {
        let dir = tempfile::tempdir().expect("tmp");
        let server = orders_server();
        let db = FakeDb::new().then_rows(vec![json!({ "status": "pending", "quantity": 2 })]);
        let ok = r#"{"name":"later","steps":[{"http":{"method":"POST","path":"/orders"},"expect":{"status":201}}]}"#;
        let found = entries(dir.path(), &[("a-create-and-pay", CREATE_AND_PAY), ("b-later", ok)]);
        let refs: Vec<&ScenarioEntry> = found.iter().collect();
        let summary = run_scenarios(dir.path(), &api(&server.url), &db, &refs);

        assert!(!summary.passed);
        let broken = &summary.scenarios[0];
        assert_eq!(broken.steps[1].status, StepStatus::Failed);
        let reason = broken.steps[1].reason.as_deref().expect("reason");
        assert!(
            reason.contains("first row: `status` expected \"created\" but got \"pending\""),
            "{reason}"
        );
        assert_eq!(broken.steps[1].actual, Some(json!([{ "status": "pending", "quantity": 2 }])));
        assert_eq!(broken.steps[2].status, StepStatus::Skipped);
        assert_eq!(broken.steps[3].status, StepStatus::Skipped);
        assert!(summary.scenarios[1].passed);
    }

    #[test]
    fn write_statements_never_reach_the_database() {
        let dir = tempfile::tempdir().expect("tmp");
        let server = orders_server();
        let db = FakeDb::new();
        let scenario = r#"{"name":"bad","steps":[
            {"db":{"sql":"DELETE FROM orders"}},
            {"db":{"tool":"anvil_select","params":{"sql":"UPDATE orders SET status='x'"}}}]}"#;
        let found = entries(dir.path(), &[("bad", scenario)]);
        let refs: Vec<&ScenarioEntry> = found.iter().collect();
        let summary = run_scenarios(dir.path(), &api(&server.url), &db, &refs);
        let step = &summary.scenarios[0].steps[0];
        assert_eq!(step.status, StepStatus::Failed);
        assert!(
            step.reason.as_deref().is_some_and(|r| r.contains("only SELECT")),
            "{:?}",
            step.reason
        );
        assert!(db.calls().is_empty());

        let second = entries(
            dir.path(),
            &[(
                "bad2",
                r#"{"name":"bad2","steps":[{"db":{"tool":"anvil_select","params":{"sql":"UPDATE orders SET status='x'"}}}]}"#,
            )],
        );
        let refs: Vec<&ScenarioEntry> = second.iter().filter(|e| e.id == "bad2").collect();
        let summary = run_scenarios(dir.path(), &api(&server.url), &db, &refs);
        assert_eq!(summary.scenarios[0].steps[0].status, StepStatus::Failed);
        assert!(db.calls().is_empty());
    }

    #[test]
    fn unknown_variables_and_unreachable_apis_fail_the_step_with_a_clear_reason() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = FakeDb::new();
        let unknown =
            r#"{"name":"u","steps":[{"http":{"method":"GET","path":"/orders/{{nope}}"}}]}"#;
        let down = r#"{"name":"d","steps":[{"http":{"method":"GET","path":"/health"}}]}"#;
        let found = entries(dir.path(), &[("a-unknown", unknown), ("b-down", down)]);
        let refs: Vec<&ScenarioEntry> = found.iter().collect();
        let summary = run_scenarios(dir.path(), &api("http://127.0.0.1:1"), &db, &refs);
        let first = summary.scenarios[0].steps[0].reason.as_deref().expect("reason");
        assert!(first.contains("unknown variable `nope`"), "{first}");
        let second = summary.scenarios[1].steps[0].reason.as_deref().expect("reason");
        assert!(second.contains("GET http://127.0.0.1:1/health failed"), "{second}");
    }

    #[test]
    fn a_toolbox_failure_fails_the_step_with_its_message() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = FakeDb::new().then_error("no such table: orders");
        let scenario = r#"{"name":"t","steps":[{"db":{"sql":"SELECT 1 FROM orders"}}]}"#;
        let found = entries(dir.path(), &[("t", scenario)]);
        let refs: Vec<&ScenarioEntry> = found.iter().collect();
        let summary = run_scenarios(dir.path(), &ApiQaConfig::default(), &db, &refs);
        let reason = summary.scenarios[0].steps[0].reason.as_deref().expect("reason");
        assert!(reason.contains("toolbox: no such table: orders"), "{reason}");
    }

    #[test]
    fn invalid_files_are_reported_as_errored_scenarios() {
        let dir = tempfile::tempdir().expect("tmp");
        let found = entries(dir.path(), &[("broken", "{")]);
        let refs: Vec<&ScenarioEntry> = found.iter().collect();
        let summary = run_scenarios(dir.path(), &ApiQaConfig::default(), &FakeDb::new(), &refs);
        assert!(!summary.passed);
        assert!(
            summary.scenarios[0]
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("invalid scenario"))
        );
    }

    #[test]
    fn the_run_token_is_unique_per_execution() {
        assert_ne!(new_run_token(), new_run_token());
    }
}
