//! `anvil deps`: Tier 2 registry health auditing across npm, PyPI, pub.dev, and Maven.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use anvil_core::git::{self, DiffScope};
use anvil_deps::{
    AuditOptions, LiveRegistryFetcher, audit_changed_files, audit_manifests, is_supported_manifest,
};

use crate::commands::ExitCodeError;
use crate::ui;

/// Executes `anvil deps [--all] [manifests...]`.
pub fn execute(root: &Path, all: bool, manifests: &[PathBuf]) -> Result<()> {
    let options = AuditOptions::from_repo(root)?;
    let fetcher = LiveRegistryFetcher;

    let report = if !manifests.is_empty() {
        audit_manifests(root, manifests, &fetcher, &options)?
    } else if all {
        let found = discover_manifests(root);
        audit_manifests(root, &found, &fetcher, &options)?
    } else {
        let changed: Vec<PathBuf> = git::changed_files(root, DiffScope::WorkingTree)
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect();
        audit_changed_files(root, &changed, &fetcher, &options)?
    };

    println!(
        "{} checked={} cache_hits={} network={} max_age={}mo findings={} errors={}",
        ui::accent("deps audit:"),
        ui::bold(&report.checked_packages.to_string()),
        report.cache_hits,
        report.network_fetches,
        report.max_age_months,
        if report.findings.is_empty() {
            ui::green("0")
        } else {
            ui::red(&report.findings.len().to_string())
        },
        report.errors.len()
    );
    for finding in &report.findings {
        let vuln_suffix = if finding.vulnerabilities.is_empty() {
            String::new()
        } else {
            format!(" vulns={:?}", finding.vulnerabilities)
        };
        println!(
            "  {} {}:{} {} issues={:?} age={:?}mo{vuln_suffix}",
            ui::warn_badge(),
            finding.ecosystem,
            ui::bold(&finding.package),
            ui::dim(&format!("({})", finding.manifest_path)),
            finding.issues,
            finding.age_months
        );
    }
    for err in &report.errors {
        eprintln!("  {} {err}", ui::red("[ERR] "));
    }
    if !report.findings.is_empty() {
        return Err(ExitCodeError::silent(1).into());
    }
    Ok(())
}

pub(crate) fn discover_manifests(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_recursive(root, root, 0, &mut out);
    out.sort();
    out
}

fn collect_recursive(root: &Path, dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let skip = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                matches!(
                    n,
                    ".git"
                        | ".harness"
                        | "node_modules"
                        | "target"
                        | "dist"
                        | "build"
                        | ".venv"
                        | "vendor"
                        | "storage"
                )
            });
            if !skip {
                collect_recursive(root, &path, depth + 1, out);
            }
        } else if is_supported_manifest(&path)
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(rel.to_path_buf());
        }
    }
}
