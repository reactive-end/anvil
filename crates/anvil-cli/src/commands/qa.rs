//! `anvil qa`: hybrid Web & Mobile visual QA onboarding, DOM evaluation, and view-tree scans.

mod api_cmds;
mod flow_cmds;

use std::fs;
use std::path::Path;

use anvil_core::config::{AnvilConfig, WebQaDriver};
use anvil_qa::QaEngine;
use anvil_qa::flows::server::ensure_app;
use anvil_qa::model::QaReport;
use anvil_qa::onboarding::{parse_mobile_driver, parse_web_driver};
use anvil_qa::report::ReportBudget;
use anvil_qa::web::{WebEvalInput, run_playwright_capture};
use anyhow::{Context, Result, bail};
use flow_cmds::Selection;

use crate::cli::subcmds::QaSubcmd;
use crate::commands::ExitCodeError;

/// Executes `anvil qa <subcommand>`.
pub fn execute(root: &Path, sub: QaSubcmd) -> Result<()> {
    let engine = QaEngine::new(root);
    match sub {
        QaSubcmd::Onboard => cmd_onboard(root, &engine),
        QaSubcmd::SelectWeb { driver } => {
            let parsed = parse_web_driver(&driver)
                .ok_or_else(|| anyhow::anyhow!("unsupported web driver: {driver}"))?;
            let cfg = engine.select_web_driver(parsed)?;
            println!("configured web QA driver: {:?}", cfg.qa_driver.web);
            for path in anvil_adapters::web_qa::apply(root, cfg.adapters)? {
                println!("updated {}", path.display());
            }
            Ok(())
        }
        QaSubcmd::SelectMobile { driver } => {
            let parsed = parse_mobile_driver(&driver)
                .ok_or_else(|| anyhow::anyhow!("unsupported mobile driver: {driver}"))?;
            let cfg = engine.select_mobile_driver(parsed)?;
            println!("configured mobile QA driver: {:?}", cfg.qa_driver.mobile);
            Ok(())
        }
        eval_sub @ QaSubcmd::EvalWeb { .. } => cmd_eval_web(root, &engine, eval_sub),
        QaSubcmd::Run { url, flow, all_flows, start, json, phase, out_dir } => {
            match (flow.as_deref(), all_flows) {
                (Some(name), _) => flow_cmds::run(root, &Selection::One(name), start, json),
                (None, true) => flow_cmds::run(root, &Selection::All, start, json),
                (None, false) => {
                    cmd_run(root, &engine, url.as_deref(), &phase, &out_dir, start, json)
                }
            }
        }
        QaSubcmd::Baseline { flow, start, json } => {
            flow_cmds::baseline(root, flow.as_deref(), start, json)
        }
        QaSubcmd::Flows => flow_cmds::list(root),
        QaSubcmd::Api { names, start, list, json } => {
            api_cmds::run(root, &names, &api_cmds::Options { start, list, json })
        }
        QaSubcmd::ScanMobile { xml, logcat, width, height } => {
            cmd_scan_mobile(root, &engine, xml.as_deref(), logcat.as_deref(), width, height)
        }
    }
}

fn cmd_onboard(root: &Path, engine: &QaEngine) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    let platform = engine.detect_platform(&config);
    match engine.onboarding_prompt(&config) {
        Some(prompt) => println!("{}", serde_json::to_string_pretty(&prompt)?),
        None => println!(
            "QA configured: platform={:?} web={:?} mobile={:?}",
            platform, config.qa_driver.web, config.qa_driver.mobile
        ),
    }
    Ok(())
}

fn cmd_eval_web(root: &Path, engine: &QaEngine, sub: QaSubcmd) -> Result<()> {
    let QaSubcmd::EvalWeb { dom_json, viewport, width, height, console_errors, page_errors } = sub
    else {
        return Ok(());
    };
    let config = AnvilConfig::load_or_default(root)?;
    let raw = read_text_tolerant(&dom_json)?;
    let cap = engine.evaluate_web_capture(WebEvalInput {
        viewport_name: &viewport,
        width,
        height,
        dom_json: &raw,
        console_errors,
        page_errors,
        screenshot_path: None,
    })?;
    let report = engine.finalize_report(
        engine.detect_platform(&config),
        Vec::new(),
        vec![cap],
        Vec::new(),
        Vec::new(),
    );
    println!("{}", engine.format_report(&report, ReportBudget::default()));
    if !report.passed {
        return Err(ExitCodeError::silent(1).into());
    }
    Ok(())
}

fn cmd_scan_mobile(
    root: &Path,
    engine: &QaEngine,
    xml_path: Option<&Path>,
    logcat_path: Option<&Path>,
    width: i32,
    height: i32,
) -> Result<()> {
    if xml_path.is_none() && logcat_path.is_none() && !root.join("app").exists() {
        bail!("provide --xml and/or --logcat for mobile QA scanning");
    }
    let config = AnvilConfig::load_or_default(root)?;
    let xml = xml_path.map(read_text_tolerant).transpose()?;
    let logcat = logcat_path.map(read_text_tolerant).transpose()?;

    let violations =
        engine.scan_mobile_artifacts(xml.as_deref(), (width, height), logcat.as_deref())?;
    let report = engine.finalize_report(
        engine.detect_platform(&config),
        Vec::new(),
        Vec::new(),
        violations,
        Vec::new(),
    );
    println!("{}", engine.format_report(&report, ReportBudget::default()));
    if !report.passed {
        return Err(ExitCodeError::silent(1).into());
    }
    Ok(())
}

fn cmd_run(
    root: &Path,
    engine: &QaEngine,
    url: Option<&str>,
    phase: &str,
    out_dir: &Path,
    start: bool,
    json: bool,
) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    match config.qa_driver.web {
        WebQaDriver::Skip => {
            println!("QA web driver is configured as `skip`; skipping `anvil qa run`");
            return Ok(());
        }
        WebQaDriver::ChromeDevTools | WebQaDriver::Browser => {
            bail!(
                "configured web QA driver `{:?}` runs via IDE/MCP + `anvil qa eval-web`; switch with `anvil qa select-web playwright` to run headless Playwright captures (flows run with `--flow`/`--all-flows` under any driver)",
                config.qa_driver.web
            );
        }
        WebQaDriver::Playwright | WebQaDriver::Unconfigured => {}
    }
    let Some(url) = url.or(config.qa.web.url.as_deref()) else {
        bail!("pass a URL or set `qa.web.url` in .harness/config.json");
    };
    let app = if start { ensure_app(root, &config.qa.web)? } else { None };
    let report = audit_url(root, engine, &config, url, phase, out_dir);
    drop(app);
    let report = report?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}", engine.format_report(&report, ReportBudget::default()));
    }
    if !report.passed {
        return Err(ExitCodeError::silent(1).into());
    }
    Ok(())
}

/// Captures `url` headlessly at both viewports and builds the DOM-audit report.
pub(crate) fn audit_url(
    root: &Path,
    engine: &QaEngine,
    config: &AnvilConfig,
    url: &str,
    phase: &str,
    out_dir: &Path,
) -> Result<QaReport> {
    let resolved_out =
        if out_dir.is_absolute() { out_dir.to_path_buf() } else { root.join(out_dir) };
    let captures = run_playwright_capture(url, phase, &resolved_out, Some(root))?;
    let (before, after) =
        if phase == "before" { (captures, Vec::new()) } else { (Vec::new(), captures) };
    Ok(engine.finalize_report(
        engine.detect_platform(config),
        before,
        after,
        Vec::new(),
        Vec::new(),
    ))
}

/// Reads UTF-8, UTF-16LE/BE (e.g. PowerShell redirection), or lossy byte logs without failing.
fn read_text_tolerant(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> =
            bytes[2..].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        return Ok(String::from_utf16_lossy(&units));
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> =
            bytes[2..].as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
        return Ok(String::from_utf16_lossy(&units));
    }
    let lossy = String::from_utf8_lossy(&bytes);
    Ok(lossy.strip_prefix('\u{feff}').unwrap_or(&lossy).to_owned())
}
