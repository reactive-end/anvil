//! Tier 2 API QA: runs the repository's scenarios against the running API.

use std::path::Path;
use std::time::Duration;

use anvil_core::config::AnvilConfig;
use anvil_qa::api::db::ToolboxClient;
use anvil_qa::api::run::run_scenarios;
use anvil_qa::api::spec::load_scenarios;
use anvil_qa::api::{scenarios_dir, summary};
use anvil_qa::flows::server::{answers, ensure_api};

use crate::ui;

/// Reachability probe timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Runs the API scenarios when `qa.api.url` is set and the change touches the API (or `full`).
/// Returns `true` when a scenario fails; an API that is not running only produces an advisory.
pub(super) fn run(root: &Path, config: &AnvilConfig, changed: &[String], full: bool) -> bool {
    let api = &config.qa.api;
    let Some(ready_url) = api.ready_url() else {
        return false;
    };
    if !full && !api.touches(changed) {
        return false;
    }
    let tag = ui::cyan("[Tier 2]");
    let entries = match load_scenarios(&scenarios_dir(root, api)) {
        Ok(entries) => entries,
        Err(err) => {
            println!("  {tag} {}", ui::red(&format!("API QA: cannot read scenarios: {err:#}")));
            return true;
        }
    };
    if entries.is_empty() {
        println!("  {tag} API QA skipped: no scenarios in {}", api.scenarios_dir);
        return false;
    }
    let mut app = None;
    if !answers(&ready_url, PROBE_TIMEOUT) {
        if api.autostart && api.start_command.is_some() {
            match ensure_api(root, api) {
                Ok(started) => app = started,
                Err(err) => {
                    println!("  {tag} API QA skipped: could not autostart the API: {err:#}");
                    return false;
                }
            }
        } else {
            println!("  {tag} API QA skipped: {ready_url} does not answer; {}", start_hint(config));
            return false;
        }
    }
    let refs: Vec<_> = entries.iter().collect();
    let run = run_scenarios(root, api, &ToolboxClient::new(root, &config.db), &refs);
    drop(app);
    for report in &run.scenarios {
        let paint = if report.passed { ui::green } else { ui::red };
        for line in summary::scenario_lines(report) {
            println!("  {tag} {}", paint(&line));
        }
    }
    if let Some(path) = &run.report_path {
        println!("  {tag} API QA report: {}", path.display());
    }
    !run.passed
}

fn start_hint(config: &AnvilConfig) -> String {
    match config.qa.api.start_command.as_deref() {
        Some(command) => {
            let cwd = config.qa.api.start_cwd.as_deref().unwrap_or(".");
            format!(
                "start it with `{command}` (in `{cwd}`), run `anvil qa api --start`, or set `qa.api.autostart`"
            )
        }
        None => "start the API, or set `qa.api.start_command` so Anvil can".to_owned(),
    }
}
