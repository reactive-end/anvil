//! Hybrid visual QA driver for Web DOM assertions and mobile view trees.

pub mod godot;
pub mod mobile;
pub mod model;
pub mod onboarding;
pub mod report;
pub mod web;

use std::path::PathBuf;

use anvil_core::config::{AnvilConfig, MobileQaDriver, WebQaDriver};
use anyhow::Result;

use crate::model::{OnboardingPrompt, QaReport, QaViolation, UiPlatform, ViewportCapture};
use crate::report::ReportBudget;
use crate::web::WebEvalInput;

/// High-level coordinator for hybrid Web and Mobile visual QA workflows.
#[derive(Debug, Clone)]
pub struct QaEngine {
    repo_root: PathBuf,
}

impl QaEngine {
    /// Creates a new [`QaEngine`] bound to `repo_root`.
    pub fn new(repo_root: impl Into<PathBuf>) -> Self {
        Self { repo_root: repo_root.into() }
    }

    /// Detects active UI platforms from [`AnvilConfig`].
    pub fn detect_platform(&self, config: &AnvilConfig) -> UiPlatform {
        onboarding::detect_ui_platform(config)
    }

    /// Builds an interactive onboarding prompt when active UI drivers are `Unconfigured`.
    pub fn onboarding_prompt(&self, config: &AnvilConfig) -> Option<OnboardingPrompt> {
        onboarding::build_onboarding_prompt(&self.repo_root, config)
    }

    /// Persists the chosen web QA driver into `.harness/config.json`.
    pub fn select_web_driver(&self, driver: WebQaDriver) -> Result<AnvilConfig> {
        onboarding::persist_web_driver(&self.repo_root, driver)
    }

    /// Persists the chosen mobile QA driver into `.harness/config.json`.
    pub fn select_mobile_driver(&self, driver: MobileQaDriver) -> Result<AnvilConfig> {
        onboarding::persist_mobile_driver(&self.repo_root, driver)
    }

    /// Evaluates raw `dom_audit.js` JSON plus console/page errors into a [`ViewportCapture`].
    pub fn evaluate_web_capture(&self, input: WebEvalInput<'_>) -> Result<ViewportCapture> {
        web::evaluate_dom_audit(input)
    }

    /// Scans optional `uiautomator` XML, `logcat` output, and Roborazzi diff directories.
    pub fn scan_mobile_artifacts(
        &self,
        uiautomator_xml: Option<&str>,
        viewport: (i32, i32),
        logcat: Option<&str>,
    ) -> Result<Vec<QaViolation>> {
        let mut out = Vec::new();
        if let Some(xml) = uiautomator_xml {
            out.extend(mobile::scan_uiautomator_xml(xml, viewport.0, viewport.1));
        }
        if let Some(logs) = logcat {
            out.extend(mobile::scan_logcat(logs));
        }
        out.extend(mobile::scan_roborazzi_diffs(&self.repo_root)?);
        Ok(out)
    }

    /// Audits a Godot `.tscn` scene tree for missing 3D camera, lighting, or physics collision shapes.
    pub fn audit_godot_scene(&self, tscn_source: &str, file: &str) -> Vec<QaViolation> {
        godot::audit_godot_scene_tree(tscn_source, file)
    }

    /// Builds a combined before/after [`QaReport`].
    pub fn finalize_report(
        &self,
        platform: UiPlatform,
        before: Vec<ViewportCapture>,
        after: Vec<ViewportCapture>,
        mobile_violations: Vec<QaViolation>,
        notes: Vec<String>,
    ) -> QaReport {
        report::build_report(platform, before, after, mobile_violations, notes)
    }

    /// Formats a token-budgeted compressed QA report for LLM consumption.
    pub fn format_report(&self, report: &QaReport, budget: ReportBudget) -> String {
        report::format_compressed_report(report, budget)
    }
}

/// Crate version marker for workspace smoke checks.
pub fn crate_name() -> &'static str {
    "anvil-qa"
}
