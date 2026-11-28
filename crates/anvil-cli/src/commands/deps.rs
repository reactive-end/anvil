//! `anvil deps`: Tier 2 registry health auditing across npm, PyPI, pub.dev, and Maven.

use std::path::{Path, PathBuf};

use anvil_core::git::{self, DiffScope};
pub(crate) use anvil_deps::discover_manifests;
use anvil_deps::{
    AuditOptions, DepFinding, LiveRegistryFetcher, audit_changed_files, audit_manifests,
};
use anyhow::Result;

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

    let blocking = report.blocking_count();
    let informational = report.findings.len() - blocking;
    println!(
        "{} checked={} cache_hits={} network={} max_age={}mo findings={} informational={} errors={}",
        ui::accent("deps audit:"),
        ui::bold(&report.checked_packages.to_string()),
        report.cache_hits,
        report.network_fetches,
        report.max_age_months,
        if blocking == 0 { ui::green("0") } else { ui::red(&blocking.to_string()) },
        informational,
        report.errors.len()
    );
    for finding in &report.findings {
        println!("{}", finding_line(finding));
    }
    for err in &report.errors {
        eprintln!("  {} {err}", ui::red("[ERR] "));
    }
    if blocking > 0 {
        return Err(ExitCodeError::silent(1).into());
    }
    Ok(())
}

/// One report row: a red `[FAIL]` badge for a blocking finding, the yellow `[WARN]` one for an
/// informational finding that does not fail the audit.
pub(crate) fn finding_line(finding: &DepFinding) -> String {
    let badge = if finding.is_blocking() { ui::fail_badge() } else { ui::warn_badge() };
    format!(
        "  {badge} {} issues={:?} age={:?}mo{}",
        ui::bold(&finding.label()),
        finding.issues,
        finding.age_months,
        advisory_suffix(finding)
    )
}

/// ` vulns=[..]` for real vulnerabilities and ` advisories=[..] (informational, non-blocking)` for the rest.
pub(crate) fn advisory_suffix(finding: &DepFinding) -> String {
    let mut out = String::new();
    if !finding.vulnerabilities.is_empty() {
        out.push_str(&format!(" vulns={:?}", finding.vulnerabilities));
    }
    if !finding.informational.is_empty() {
        out.push_str(&format!(
            " advisories={:?} (informational, non-blocking)",
            finding.informational
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use anvil_deps::{DepIssueKind, Ecosystem};

    use super::*;

    fn finding(vulns: &[&str], info: &[&str]) -> DepFinding {
        DepFinding {
            ecosystem: Ecosystem::Cargo,
            package: "chrono".to_owned(),
            manifest_path: "Cargo.toml".to_owned(),
            issues: vec![DepIssueKind::Informational],
            age_months: None,
            last_publish: None,
            deprecation_message: None,
            latest_version: None,
            vulnerabilities: vulns.iter().map(|s| (*s).to_owned()).collect(),
            informational: info.iter().map(|s| (*s).to_owned()).collect(),
            resolved_version: Some("0.4.45".to_owned()),
            lockfile: Some("Cargo.lock".to_owned()),
            version_req: Some("0.4".to_owned()),
        }
    }

    #[test]
    fn suffix_separates_vulnerabilities_from_informational_advisories() {
        assert_eq!(advisory_suffix(&finding(&[], &[])), "");
        assert_eq!(advisory_suffix(&finding(&["GHSA-1"], &[])), " vulns=[\"GHSA-1\"]");
        let both = advisory_suffix(&finding(&["GHSA-1"], &["RUSTSEC-2026-0543"]));
        assert!(both.contains("vulns=[\"GHSA-1\"]"));
        assert!(both.contains("advisories=[\"RUSTSEC-2026-0543\"] (informational, non-blocking)"));
    }

    #[test]
    fn blocking_findings_get_the_fail_badge_and_informational_ones_the_warn_badge() {
        let informational = finding(&[], &["RUSTSEC-2026-0543"]);
        let mut blocking = finding(&["GHSA-1"], &[]);
        blocking.issues = vec![DepIssueKind::Vulnerable];

        let warn_row = finding_line(&informational);
        let fail_row = finding_line(&blocking);

        assert!(warn_row.contains("[WARN]") && !warn_row.contains("[FAIL]"), "{warn_row}");
        assert!(fail_row.contains("[FAIL]") && !fail_row.contains("[WARN]"), "{fail_row}");
    }

    #[test]
    fn finding_label_shows_resolved_version_and_lockfile() {
        assert_eq!(finding(&[], &[]).label(), "cargo:chrono 0.4.45 (Cargo.lock)");
    }
}
