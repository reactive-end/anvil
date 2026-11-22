//! `anvil qa`: hybrid Web & Mobile visual QA onboarding, DOM evaluation, and view-tree scans.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use anvil_core::config::{AnvilConfig, WebQaDriver};
use anvil_qa::QaEngine;
use anvil_qa::onboarding::{parse_mobile_driver, parse_web_driver};
use anvil_qa::report::ReportBudget;
use anvil_qa::web::{WebEvalInput, run_playwright_capture};

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
        QaSubcmd::Run {
            url,
            phase,
            out_dir,
        } => cmd_run(root, &engine, &url, &phase, &out_dir),
        QaSubcmd::ScanMobile {
            xml,
            logcat,
            width,
            height,
        } => cmd_scan_mobile(
            root,
            &engine,
            xml.as_deref(),
            logcat.as_deref(),
            width,
            height,
        ),
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
    let QaSubcmd::EvalWeb {
        dom_json,
        viewport,
        width,
        height,
        console_errors,
        page_errors,
    } = sub
    else {
        return Ok(());
    };
    let config = AnvilConfig::load_or_default(root)?;
    let raw = fs::read_to_string(&dom_json)
        .with_context(|| format!("failed to read {}", dom_json.display()))?;
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

    let violations = engine.scan_mobile_artifacts(
        xml.as_deref(),
        (width, height),
        logcat.as_deref(),
    )?;
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

fn cmd_run(root: &Path, engine: &QaEngine, url: &str, phase: &str, out_dir: &Path) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    if config.qa_driver.web == WebQaDriver::Skip {
        println!("QA web driver is configured as `skip`; skipping `anvil qa run`");
        return Ok(());
    }
    let resolved_out = if out_dir.is_absolute() {
        out_dir.to_path_buf()
    } else {
        root.join(out_dir)
    };
    let captures = run_playwright_capture(url, phase, &resolved_out)?;
    let report = engine.finalize_report(
        engine.detect_platform(&config),
        Vec::new(),
        captures,
        Vec::new(),
        Vec::new(),
    );
    println!("{}", engine.format_report(&report, ReportBudget::default()));
    if !report.passed {
        return Err(ExitCodeError::silent(1).into());
    }
    Ok(())
}

/// Reads UTF-8, UTF-16LE/BE (e.g. PowerShell redirection), or lossy byte logs without failing.
fn read_text_tolerant(path: &Path) -> Result<String> {
    let bytes =
        fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        return Ok(String::from_utf16_lossy(&units));
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect();
        return Ok(String::from_utf16_lossy(&units));
    }
    let lossy = String::from_utf8_lossy(&bytes);
    Ok(lossy.strip_prefix('\u{feff}').unwrap_or(&lossy).to_owned())
}
