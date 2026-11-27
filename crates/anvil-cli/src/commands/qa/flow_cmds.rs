//! `anvil qa run --flow|--all-flows`, `anvil qa baseline`, and `anvil qa flows`.

use std::path::Path;

use anvil_core::config::{AnvilConfig, WebQaDriver};
use anvil_qa::flows::report::RunSummary;
use anvil_qa::flows::run::{baseline_path, can_store_baselines, run_flows, store_baselines};
use anvil_qa::flows::server::ensure_app;
use anvil_qa::flows::spec::{FlowEntry, Step, find_flow, load_flows};
use anvil_qa::flows::{flows_dir, summary};
use anyhow::{Result, bail};
use serde_json::json;

use crate::commands::ExitCodeError;

/// Which flows a command works on.
pub enum Selection<'a> {
    One(&'a str),
    All,
}

/// Runs the selected flows and fails (exit 1) when any expectation, error, or visual diff fails.
pub fn run(root: &Path, selection: &Selection<'_>, start: bool, json: bool) -> Result<()> {
    let Some(summary) = execute(root, selection, start)? else {
        return Ok(());
    };
    print_summary(&summary, json, None)?;
    if summary.passed { Ok(()) } else { Err(ExitCodeError::silent(1).into()) }
}

/// Runs flows and stores their screenshots as baselines.
pub fn baseline(root: &Path, flow: Option<&str>, start: bool, json: bool) -> Result<()> {
    let selection = flow.map_or(Selection::All, Selection::One);
    let Some(summary) = execute(root, &selection, start)? else {
        return Ok(());
    };
    let mut stored = Vec::new();
    let mut refused = Vec::new();
    for report in &summary.flows {
        if can_store_baselines(report) {
            stored.extend(store_baselines(root, report)?);
        } else {
            refused.push(report.id.clone());
        }
    }
    print_summary(&summary, json, Some(&stored))?;
    if !json {
        for path in &stored {
            println!("baseline stored: {}", path.display());
        }
        if stored.is_empty() && refused.is_empty() {
            println!("no flow has a `screenshot` step, so there is nothing to store");
        }
        for id in &refused {
            println!("no baseline stored for `{id}`: the flow itself failed");
        }
    }
    if refused.is_empty() { Ok(()) } else { Err(ExitCodeError::silent(1).into()) }
}

/// Lists the flows of the repository and validates each file.
pub fn list(root: &Path) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    let dir = flows_dir(root, &config.qa.web);
    let entries = load_flows(&dir)?;
    if entries.is_empty() {
        println!(
            "no flows in {} (add `<name>.json` files; see the README \"Web QA\" section)",
            dir.display()
        );
        return Ok(());
    }
    let mut invalid = 0;
    for entry in &entries {
        match &entry.flow {
            Ok(flow) => {
                let shots: Vec<&str> = flow
                    .steps
                    .iter()
                    .filter_map(|s| {
                        if let Step::Screenshot(shot) = s { Some(shot.name()) } else { None }
                    })
                    .collect();
                let stored =
                    shots.iter().filter(|n| baseline_path(root, &entry.id, n).is_file()).count();
                println!(
                    "ok       {:<24} {} step(s), {} viewport, baselines {stored}/{}",
                    entry.id,
                    flow.steps.len(),
                    flow.viewport.dimensions().0,
                    shots.len()
                );
                if config.qa.web.url.is_none() && uses_relative_urls(flow) {
                    println!(
                        "         note: relative URLs need `qa.web.url` in .harness/config.json"
                    );
                }
            }
            Err(err) => {
                invalid += 1;
                println!("INVALID  {:<24} {err}", entry.id);
            }
        }
    }
    if invalid > 0 {
        eprintln!("{invalid} flow file(s) are invalid");
        return Err(ExitCodeError::silent(1).into());
    }
    Ok(())
}

fn uses_relative_urls(flow: &anvil_qa::flows::spec::Flow) -> bool {
    let relative = |url: &str| !(url.starts_with("http://") || url.starts_with("https://"));
    flow.url.as_deref().is_some_and(relative)
        || flow.steps.iter().any(|s| matches!(s, Step::Goto(url) if relative(url)))
}

/// Loads, selects, and runs flows; `None` when the web QA driver is `skip`.
fn execute(root: &Path, selection: &Selection<'_>, start: bool) -> Result<Option<RunSummary>> {
    let config = AnvilConfig::load_or_default(root)?;
    if config.qa_driver.web == WebQaDriver::Skip {
        println!("QA web driver is configured as `skip`; not running flows");
        return Ok(None);
    }
    let web = &config.qa.web;
    let entries = load_flows(&flows_dir(root, web))?;
    let chosen = select(&entries, selection, &flows_dir(root, web))?;
    let app = if start { ensure_app(root, web)? } else { None };
    let summary = run_flows(root, web, &chosen);
    drop(app);
    Ok(Some(summary))
}

fn select<'a>(
    entries: &'a [FlowEntry],
    selection: &Selection<'_>,
    dir: &Path,
) -> Result<Vec<&'a FlowEntry>> {
    match selection {
        Selection::All if entries.is_empty() => bail!("no flows in {}", dir.display()),
        Selection::All => Ok(entries.iter().collect()),
        Selection::One(name) => match find_flow(entries, name) {
            Some(entry) => Ok(vec![entry]),
            None => {
                let known: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
                bail!("no flow named `{name}` in {} (found: {})", dir.display(), known.join(", "))
            }
        },
    }
}

fn print_summary(
    summary: &RunSummary,
    as_json: bool,
    stored: Option<&[std::path::PathBuf]>,
) -> Result<()> {
    if as_json {
        let value = match stored {
            Some(paths) => json!({ "baselines_stored": paths, "run": summary }),
            None => serde_json::to_value(summary)?,
        };
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{}", summary::format_run(summary));
    }
    Ok(())
}
