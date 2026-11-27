//! Informational advisories (unmaintained/unsound/notice) warn but never block Tier 2.

use std::fs;
use std::path::PathBuf;

use anvil_deps::{
    AuditOptions, DepIssueKind, DependencySpec, Ecosystem, PackageHealthInfo, RegistryFetcher,
    audit_manifests,
};
use anyhow::Result;
use chrono::Utc;
use rustc_hash::{FxHashMap, FxHashSet};
use tempfile::TempDir;

struct AdvisoryFetcher {
    /// package -> advisory ids returned by OSV.
    advisories: Vec<(&'static str, Vec<&'static str>)>,
    informational: Vec<&'static str>,
}

impl RegistryFetcher for AdvisoryFetcher {
    fn fetch(&self, ecosystem: Ecosystem, package: &str) -> Result<PackageHealthInfo> {
        Ok(PackageHealthInfo {
            ecosystem,
            name: package.to_owned(),
            latest_version: Some("1.0.0".to_owned()),
            last_publish: Some(Utc::now()),
            deprecated: false,
            deprecation_message: None,
            archived: false,
            repository_url: None,
            vulnerabilities: Vec::new(),
            informational: Vec::new(),
        })
    }

    fn fetch_vulnerabilities(&self, specs: &[DependencySpec]) -> FxHashMap<usize, Vec<String>> {
        let mut out = FxHashMap::default();
        for (idx, spec) in specs.iter().enumerate() {
            if let Some((_, ids)) = self.advisories.iter().find(|(name, _)| *name == spec.name) {
                out.insert(idx, ids.iter().map(|id| (*id).to_owned()).collect());
            }
        }
        out
    }

    fn fetch_informational_advisories(&self, ids: &[String]) -> FxHashSet<String> {
        ids.iter().filter(|id| self.informational.contains(&id.as_str())).cloned().collect()
    }
}

fn audit(fetcher: &AdvisoryFetcher) -> anvil_deps::DepsAuditReport {
    let tmp = TempDir::new().expect("tempdir");
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[dependencies]\nchrono = \"0.4\"\nanyhow = \"1\"\nregex = \"1\"\n",
    )
    .expect("manifest");
    audit_manifests(tmp.path(), &[PathBuf::from("Cargo.toml")], fetcher, &AuditOptions::default())
        .expect("audit")
}

#[test]
fn unmaintained_advisory_is_informational_and_non_blocking() {
    let fetcher = AdvisoryFetcher {
        advisories: vec![("chrono", vec!["RUSTSEC-2026-0543"])],
        informational: vec!["RUSTSEC-2026-0543"],
    };
    let report = audit(&fetcher);

    assert_eq!(report.findings.len(), 1);
    let finding = &report.findings[0];
    assert_eq!(finding.issues, vec![DepIssueKind::Informational]);
    assert_eq!(finding.informational, vec!["RUSTSEC-2026-0543".to_owned()]);
    assert!(finding.vulnerabilities.is_empty());
    assert!(!finding.is_blocking());
    assert_eq!(report.blocking_count(), 0);
}

#[test]
fn real_vulnerability_stays_blocking_and_mixed_ids_are_split() {
    let fetcher = AdvisoryFetcher {
        advisories: vec![
            ("regex", vec!["RUSTSEC-2022-0013"]),
            ("chrono", vec!["RUSTSEC-2026-0543", "GHSA-xxxx-yyyy-zzzz"]),
        ],
        informational: vec!["RUSTSEC-2026-0543"],
    };
    let report = audit(&fetcher);

    assert_eq!(report.findings.len(), 2);
    assert_eq!(report.blocking_count(), 2);
    let chrono = report.findings.iter().find(|f| f.package == "chrono").expect("chrono");
    assert_eq!(chrono.issues, vec![DepIssueKind::Vulnerable, DepIssueKind::Informational]);
    assert_eq!(chrono.vulnerabilities, vec!["GHSA-xxxx-yyyy-zzzz".to_owned()]);
    assert_eq!(chrono.informational, vec!["RUSTSEC-2026-0543".to_owned()]);
    let regex = report.findings.iter().find(|f| f.package == "regex").expect("regex");
    assert_eq!(regex.issues, vec![DepIssueKind::Vulnerable]);
}
