//! `anvil qa api`: runs (or lists) the API scenarios of the repository.

use std::path::Path;
use std::time::Duration;

use anvil_core::config::AnvilConfig;
use anvil_core::config::qa::ApiQaConfig;
use anvil_qa::api::db::ToolboxClient;
use anvil_qa::api::run::run_scenarios;
use anvil_qa::api::spec::{Scenario, ScenarioEntry, find_scenario, load_scenarios};
use anvil_qa::api::{scenarios_dir, summary};
use anvil_qa::flows::server::{answers, ensure_api};
use anyhow::{Result, bail};

use crate::commands::ExitCodeError;

/// Reachability probe timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Flags of `anvil qa api`.
pub struct Options {
    pub start: bool,
    pub list: bool,
    pub json: bool,
}

/// Runs the named scenarios (all when `names` is empty); exits 1 when a step fails, a file is
/// invalid, or the API cannot be reached.
pub fn run(root: &Path, names: &[String], options: &Options) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    let api = &config.qa.api;
    let dir = scenarios_dir(root, api);
    let entries = load_scenarios(&dir)?;
    if options.list {
        return list(&entries, api, &dir);
    }
    let chosen = select(&entries, names, &dir)?;
    let app = if options.start {
        ensure_api(root, api)?
    } else {
        require_running(api)?;
        None
    };
    let client = ToolboxClient::new(root, &config.db);
    let run = run_scenarios(root, api, &client, &chosen);
    drop(app);
    if options.json {
        println!("{}", serde_json::to_string_pretty(&run)?);
    } else {
        println!("{}", summary::format_run(&run));
    }
    if run.passed { Ok(()) } else { Err(ExitCodeError::silent(1).into()) }
}

/// Without `--start` the API must already answer; a refused connection is one clear message
/// instead of the same error on every step.
fn require_running(api: &ApiQaConfig) -> Result<()> {
    let Some(ready_url) = api.ready_url() else { return Ok(()) };
    if answers(&ready_url, PROBE_TIMEOUT) {
        return Ok(());
    }
    let hint = match api.start_command.as_deref() {
        Some(command) => format!("start it with `{command}` or pass --start"),
        None => "start it, or set `qa.api.start_command` and pass --start".to_owned(),
    };
    Err(ExitCodeError::with_message(1, format!("the API does not answer at {ready_url}; {hint}"))
        .into())
}

fn select<'a>(
    entries: &'a [ScenarioEntry],
    names: &[String],
    dir: &Path,
) -> Result<Vec<&'a ScenarioEntry>> {
    if names.is_empty() {
        if entries.is_empty() {
            bail!(
                "no scenarios in {} (add `<name>.json` files; see the README \"API QA and database checks\" section)",
                dir.display()
            );
        }
        return Ok(entries.iter().collect());
    }
    names
        .iter()
        .map(|name| {
            find_scenario(entries, name).ok_or_else(|| {
                let known: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
                anyhow::anyhow!(
                    "no scenario named `{name}` in {} (found: {})",
                    dir.display(),
                    known.join(", ")
                )
            })
        })
        .collect()
}

fn list(entries: &[ScenarioEntry], api: &ApiQaConfig, dir: &Path) -> Result<()> {
    if entries.is_empty() {
        println!("no scenarios in {} (add `<name>.json` files)", dir.display());
        return Ok(());
    }
    let mut invalid = 0;
    for entry in entries {
        match &entry.scenario {
            Ok(scenario) => {
                println!("ok       {:<28} {}", entry.id, describe(scenario));
                if api.url.is_none() && has_relative_paths(scenario) {
                    println!(
                        "         note: relative paths need `qa.api.url` in .harness/config.json"
                    );
                }
            }
            Err(err) => {
                invalid += 1;
                println!("INVALID  {:<28} {err}", entry.id);
            }
        }
    }
    if invalid > 0 {
        eprintln!("{invalid} scenario file(s) are invalid");
        return Err(ExitCodeError::silent(1).into());
    }
    Ok(())
}

fn describe(scenario: &Scenario) -> String {
    let db = scenario.steps.iter().filter(|s| s.db.is_some()).count();
    format!("{} step(s), {db} database check(s)", scenario.steps.len())
}

fn has_relative_paths(scenario: &Scenario) -> bool {
    scenario
        .steps
        .iter()
        .filter_map(|s| s.http.as_ref())
        .any(|http| !(http.path.starts_with("http://") || http.path.starts_with("https://")))
}
