//! Tier 2 web QA: runs the repository's flows (or the DOM audit) against the running app.

use std::path::Path;
use std::time::Duration;

use anvil_core::config::{AnvilConfig, WebQaDriver};
use anvil_qa::QaEngine;
use anvil_qa::flows::run::run_flows;
use anvil_qa::flows::server::{answers, ensure_app};
use anvil_qa::flows::spec::load_flows;
use anvil_qa::flows::{flows_dir, summary};
use anvil_qa::report::ReportBudget;

use crate::commands::qa::audit_url;
use crate::ui;

/// Reachability probe timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Runs web QA when `qa.web.url` is set. Returns `true` when a flow or the audit fails; an app that
/// is not running only produces an advisory.
pub(super) fn run(root: &Path, config: &AnvilConfig) -> bool {
    let web = &config.qa.web;
    let Some(ready_url) = web.ready_url() else {
        return false;
    };
    if config.qa_driver.web == WebQaDriver::Skip {
        return false;
    }
    let tag = ui::cyan("[Tier 2]");
    let mut app = None;
    if !answers(&ready_url, PROBE_TIMEOUT) {
        if web.autostart && web.start_command.is_some() {
            match ensure_app(root, web) {
                Ok(started) => app = started,
                Err(err) => {
                    println!("  {tag} web QA skipped: could not autostart the app: {err:#}");
                    return false;
                }
            }
        } else {
            println!("  {tag} web QA skipped: {ready_url} does not answer; {}", start_hint(config));
            return false;
        }
    }
    let failed = run_checks(root, config, &tag);
    drop(app);
    failed
}

fn start_hint(config: &AnvilConfig) -> String {
    match config.qa.web.start_command.as_deref() {
        Some(command) => {
            let cwd = config.qa.web.start_cwd.as_deref().unwrap_or(".");
            format!(
                "start it with `{command}` (in `{cwd}`) or run `anvil qa run --all-flows --start`"
            )
        }
        None => "start the app, or set `qa.web.start_command` so Anvil can".to_owned(),
    }
}

fn run_checks(root: &Path, config: &AnvilConfig, tag: &str) -> bool {
    let web = &config.qa.web;
    let entries = match load_flows(&flows_dir(root, web)) {
        Ok(entries) => entries,
        Err(err) => {
            println!("  {tag} {}", ui::red(&format!("web QA: cannot read flows: {err:#}")));
            return true;
        }
    };
    if entries.is_empty() {
        return run_audit(root, config, tag);
    }
    let refs: Vec<_> = entries.iter().collect();
    let run = run_flows(root, web, &refs);
    for report in &run.flows {
        let paint = if report.passed { ui::green } else { ui::red };
        for line in summary::flow_lines(report) {
            println!("  {tag} {}", paint(&line));
        }
    }
    println!("  {tag} web QA artifacts: {}", run.run_dir.display());
    !run.passed
}

fn run_audit(root: &Path, config: &AnvilConfig, tag: &str) -> bool {
    if !matches!(config.qa_driver.web, WebQaDriver::Playwright | WebQaDriver::Unconfigured) {
        println!(
            "  {tag} web QA: no flows in {}; drive the checks through the MCP browser or add flows",
            config.qa.web.flows_dir
        );
        return false;
    }
    let Some(url) = config.qa.web.url.as_deref() else {
        return false;
    };
    let engine = QaEngine::new(root);
    match audit_url(root, &engine, config, url, "after", Path::new(".harness/.state/qa")) {
        Ok(report) => {
            let text = engine.format_report(&report, ReportBudget::default());
            let painted = if report.passed { ui::green(&text) } else { ui::red(&text) };
            println!("  {tag} web QA (DOM audit, no flows defined):\n{painted}");
            !report.passed
        }
        Err(err) => {
            println!("  {tag} web QA audit could not run: {err:#}");
            false
        }
    }
}
