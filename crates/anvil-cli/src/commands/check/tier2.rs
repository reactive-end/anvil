//! Tier 2 dependency health and visual QA verification helpers for `anvil check`.

use std::path::{Path, PathBuf};

use anvil_core::config::AnvilConfig;
use anvil_core::diff_classifier::DiffClassification;
use anvil_deps::{AuditOptions, LiveRegistryFetcher, audit_manifests, is_supported_manifest};
use anvil_qa::QaEngine;

use crate::commands::deps::discover_manifests;
use crate::ui;

/// Runs Tier 2 dependency auditing, visual QA readiness, and opt-in SonarQube Quality Gate checks.
/// Returns `true` when blocking findings or a failing Sonar Quality Gate are reported.
pub fn run_tier2(
    root: &Path,
    config: &AnvilConfig,
    changed: &[String],
    classification: &DiffClassification,
    full: bool,
    run_sonar: bool,
) -> bool {
    let mut failed = false;
    if classification.needs_deps_audit || full {
        failed |= run_deps_audit(root, changed, full);
    }
    if run_sonar {
        failed |= crate::commands::sonar::run_tier2_quality_gate(root, config);
    }
    if classification.needs_visual_qa || full {
        let qa = QaEngine::new(root);
        if let Some(prompt) = qa.onboarding_prompt(config) {
            println!(
                "  {} visual QA driver unconfigured for {:?}; run `anvil qa onboard`",
                ui::yellow("[Tier 2]"),
                prompt.platform
            );
        } else {
            println!(
                "  {} visual QA platform={:?}",
                ui::cyan("[Tier 2]"),
                qa.detect_platform(config)
            );
        }
    }
    failed
}

fn run_deps_audit(root: &Path, changed: &[String], full: bool) -> bool {
    let mut manifests: Vec<PathBuf> = changed
        .iter()
        .map(PathBuf::from)
        .filter(|p| is_supported_manifest(p))
        .collect();
    if manifests.is_empty() && full {
        manifests = discover_manifests(root);
    }
    if manifests.is_empty() {
        return false;
    }

    let options = match AuditOptions::from_repo(root) {
        Ok(opts) => opts,
        Err(err) => {
            eprintln!("  [Tier 2] deps audit config error: {err:#}");
            return false;
        }
    };
    match audit_manifests(root, &manifests, &LiveRegistryFetcher, &options) {
        Ok(report) => {
            for err in &report.errors {
                eprintln!("  [Tier 2] deps audit error: {err}");
            }
            if report.findings.is_empty() {
                println!(
                    "  {} {}",
                    ui::cyan("[Tier 2]"),
                    ui::green(&format!(
                        "deps audit: PASS (checked={}, 0 findings)",
                        report.checked_packages
                    ))
                );
                return false;
            }
            println!(
                "  {} {}",
                ui::cyan("[Tier 2]"),
                ui::red(&format!("deps audit: FAIL ({} finding(s))", report.findings.len()))
            );
            for f in &report.findings {
                let detail = f
                    .deprecation_message
                    .as_deref()
                    .map(|m| format!(" detail={m}"))
                    .unwrap_or_default();
                println!(
                    "    - {}:{} ({}) issues={:?} age={:?}mo{detail}",
                    f.ecosystem, f.package, f.manifest_path, f.issues, f.age_months
                );
            }
            true
        }
        Err(err) => {
            eprintln!("  [Tier 2] deps audit failed: {err:#}");
            false
        }
    }
}
