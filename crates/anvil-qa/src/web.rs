//! Web DOM assertion evaluation, Playwright/CDP runner command builders, and capture ingestion.

mod playwright;

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::model::{QaViolation, QaViolationKind, Rect, ViewportCapture};

pub use playwright::{
    RunnerCommand, build_playwright_command, build_playwright_runner_script, run_playwright_capture,
};

/// Embedded standalone browser DOM audit script (`scripts/dom_audit.js`).
pub const DOM_AUDIT_SCRIPT: &str = include_str!("../scripts/dom_audit.js");

/// Standard desktop viewport dimensions (`1440x900`).
pub const DESKTOP_VIEWPORT: (&str, u32, u32) = ("desktop", 1440, 900);

/// Standard mobile viewport dimensions (`390x844`).
pub const MOBILE_VIEWPORT: (&str, u32, u32) = ("mobile", 390, 844);

/// Input payload for evaluating a single viewport DOM audit and runtime logs.
#[derive(Debug, Clone)]
pub struct WebEvalInput<'a> {
    pub viewport_name: &'a str,
    pub width: u32,
    pub height: u32,
    pub dom_json: &'a str,
    pub console_errors: Vec<String>,
    pub page_errors: Vec<String>,
    pub screenshot_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RawDomAudit {
    #[allow(dead_code)]
    passed: bool,
    #[serde(default)]
    violations: Vec<RawDomViolation>,
}

#[derive(Debug, Deserialize)]
struct RawDomViolation {
    kind: QaViolationKind,
    selector: String,
    message: String,
    rect: Option<RawRect>,
}

#[derive(Debug, Deserialize)]
struct RawRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// Evaluates raw `dom_audit.js` JSON plus console/page errors into a [`ViewportCapture`].
pub fn evaluate_dom_audit(input: WebEvalInput<'_>) -> Result<ViewportCapture> {
    let raw: RawDomAudit = serde_json::from_str(input.dom_json)
        .context("failed to parse dom_audit.js JSON output")?;
    Ok(build_capture(
        input.viewport_name,
        input.width,
        input.height,
        raw,
        input.console_errors,
        input.page_errors,
        input.screenshot_path,
    ))
}

/// Builds a CDP `Runtime.evaluate` payload running the embedded DOM audit script.
pub fn build_cdp_evaluate_payload() -> serde_json::Value {
    serde_json::json!({
        "method": "Runtime.evaluate",
        "params": {
            "expression": DOM_AUDIT_SCRIPT,
            "returnByValue": true
        }
    })
}

pub(super) fn build_capture(
    viewport_name: &str,
    width: u32,
    height: u32,
    raw: RawDomAudit,
    console_errors: Vec<String>,
    page_errors: Vec<String>,
    screenshot_path: Option<PathBuf>,
) -> ViewportCapture {
    let vp = Some(viewport_name.to_owned());
    let mut violations: Vec<QaViolation> = raw
        .violations
        .into_iter()
        .map(|v| QaViolation {
            kind: v.kind,
            selector: v.selector,
            message: v.message,
            rect: v.rect.map(|r| Rect {
                x: r.x.round() as i32,
                y: r.y.round() as i32,
                width: r.width.round() as i32,
                height: r.height.round() as i32,
            }),
            viewport: vp.clone(),
        })
        .collect();

    for err in &console_errors {
        violations.push(QaViolation {
            kind: QaViolationKind::ConsoleError,
            selector: "console".to_owned(),
            message: err.clone(),
            rect: None,
            viewport: vp.clone(),
        });
    }
    for err in &page_errors {
        violations.push(QaViolation {
            kind: QaViolationKind::PageError,
            selector: "window".to_owned(),
            message: err.clone(),
            rect: None,
            viewport: vp.clone(),
        });
    }

    ViewportCapture {
        name: viewport_name.to_owned(),
        width,
        height,
        screenshot_path,
        violations,
        console_errors,
        page_errors,
    }
}
