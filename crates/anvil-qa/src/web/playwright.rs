//! Playwright headless runner script generation and optional execution.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::{DOM_AUDIT_SCRIPT, RawDomAudit, build_capture};
use crate::model::ViewportCapture;

/// Command specification returned by runner builders for deterministic testing and execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerCommand {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawPlaywrightOutput {
    captures: Vec<RawPlaywrightCapture>,
}

#[derive(Debug, Deserialize)]
struct RawPlaywrightCapture {
    name: String,
    width: u32,
    height: u32,
    screenshot_path: Option<PathBuf>,
    audit: RawDomAudit,
    #[serde(default)]
    console_errors: Vec<String>,
    #[serde(default)]
    page_errors: Vec<String>,
}

fn validate_phase(phase: &str) -> Result<&str> {
    match phase {
        "before" | "after" => Ok(phase),
        _ => bail!("unsupported QA phase `{phase}` (expected `before` or `after`)"),
    }
}

/// Generates a standalone Node+Playwright runner script for `desktop` (1440x900) and `mobile` (390x844).
pub fn build_playwright_runner_script(url: &str, phase: &str, out_dir: &Path) -> Result<String> {
    let phase = validate_phase(phase)?;
    let url_json = serde_json::to_string(url)?;
    let phase_json = serde_json::to_string(phase)?;
    let out_json = serde_json::to_string(&out_dir.to_string_lossy().into_owned())?;
    let audit_iife = DOM_AUDIT_SCRIPT.trim().trim_end_matches(';');

    Ok(format!(
        r#"import fs from "node:fs";
import path from "node:path";
import {{ createRequire }} from "node:module";

async function loadPlaywright() {{
  try {{
    const req = createRequire(path.join(process.cwd(), "package.json"));
    return req("playwright");
  }} catch {{
    return await import("playwright");
  }}
}}

const {{ chromium }} = await loadPlaywright();

const url = {url_json};
const phase = {phase_json};
const outDir = {out_json};
const runDomAudit = () => ({audit_iife});
const viewports = [
  {{ name: "desktop", width: 1440, height: 900 }},
  {{ name: "mobile", width: 390, height: 844 }},
];

fs.mkdirSync(outDir, {{ recursive: true }});
const browser = await chromium.launch({{ headless: true }});
const captures = [];
try {{
  for (const vp of viewports) {{
    const page = await browser.newPage({{ viewport: {{ width: vp.width, height: vp.height }} }});
    const console_errors = [];
    const page_errors = [];
    page.on("console", (msg) => {{ if (msg.type() === "error") console_errors.push(msg.text()); }});
    page.on("pageerror", (err) => page_errors.push(String(err)));
    await page.goto(url, {{ waitUntil: "networkidle" }});
    const audit = await page.evaluate(runDomAudit);
    const shot = path.join(outDir, `${{phase}}-${{vp.name}}.png`);
    await page.screenshot({{ path: shot, fullPage: true }});
    captures.push({{ name: vp.name, width: vp.width, height: vp.height, screenshot_path: shot, audit, console_errors, page_errors }});
    await page.close();
  }}
}} finally {{
  await browser.close();
}}
console.log(JSON.stringify({{ captures }}));
"#
    ))
}

/// Builds the OS command to run a generated Playwright capture script via `node`.
pub fn build_playwright_command(script_path: &Path) -> RunnerCommand {
    RunnerCommand {
        program: "node".to_owned(),
        args: vec![script_path.to_string_lossy().into_owned()],
    }
}

/// Runs headless Playwright capture for `desktop` + `mobile` viewports when Playwright is installed.
pub fn run_playwright_capture(
    url: &str,
    phase: &str,
    out_dir: &Path,
) -> Result<Vec<ViewportCapture>> {
    let phase = validate_phase(phase)?;
    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create QA output dir {}", out_dir.display()))?;
    let script_path = out_dir.join(format!("anvil-qa-{phase}.mjs"));
    let script = build_playwright_runner_script(url, phase, out_dir)?;
    fs::write(&script_path, script)
        .with_context(|| format!("failed to write {}", script_path.display()))?;

    let cmd = build_playwright_command(&script_path);
    let output = Command::new(&cmd.program)
        .args(&cmd.args)
        .output()
        .with_context(|| format!("failed to invoke `{}` for Playwright QA", cmd.program))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Playwright QA runner failed: {}", stderr.trim());
    }

    let parsed: RawPlaywrightOutput = serde_json::from_slice(&output.stdout)
        .context("failed to parse Playwright runner JSON output")?;

    Ok(parsed
        .captures
        .into_iter()
        .map(|c| {
            build_capture(
                &c.name,
                c.width,
                c.height,
                c.audit,
                c.console_errors,
                c.page_errors,
                c.screenshot_path,
            )
        })
        .collect())
}
