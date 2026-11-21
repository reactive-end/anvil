//! Tier 2 dependency health and visual QA verification helpers for `anvil check`.

use std::path::{Path, PathBuf};

use anvil_core::config::AnvilConfig;
use anvil_core::diff_classifier::DiffClassification;
use anvil_deps::{AuditOptions, LiveRegistryFetcher, audit_manifests, is_supported_manifest};
use anvil_qa::QaEngine;

use crate::commands::deps::discover_manifests;

/// Runs Tier 2 dependency auditing and visual QA readiness checks.
pub fn run_tier2(
    root: &Path,
    config: &AnvilConfig,
    changed: &[String],
    classification: &DiffClassification,
    full: bool,
) {
    if classification.needs_deps_audit || full {
        run_deps_audit(root, changed, full);
    }
    if classification.needs_visual_qa || full {
        let qa = QaEngine::new(root);
        if let Some(prompt) = qa.onboarding_prompt(config) {
            println!(
                "  [Tier 2] visual QA driver unconfigured for {:?}; run `anvil qa onboard`",
                prompt.platform
            );
        } else {
            println!("  [Tier 2] visual QA platform={:?}", qa.detect_platform(config));
        }
    }
}

fn run_deps_audit(root: &Path, changed: &[String], full: bool) {
    let mut manifests: Vec<PathBuf> = changed
        .iter()
        .map(PathBuf::from)
        .filter(|p| is_supported_manifest(p))
        .collect();
    if manifests.is_empty() && full {
        manifests = discover_manifests(root);
    }
    if manifests.is_empty() {
        return;
    }

    let options = match AuditOptions::from_repo(root) {
        Ok(opts) => opts,
        Err(err) => {
            eprintln!("  [Tier 2] deps audit config error: {err:#}");
            return;
        }
    };
    match audit_manifests(root, &manifests, &LiveRegistryFetcher, &options) {
        Ok(report) => {
            if !report.findings.is_empty() {
                println!("  [Tier 2] deps audit: {} finding(s)", report.findings.len());
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
            }
            for err in &report.errors {
                eprintln!("  [Tier 2] deps audit error: {err}");
            }
        }
        Err(err) => {
            eprintln!("  [Tier 2] deps audit failed: {err:#}");
        }
    }
}
