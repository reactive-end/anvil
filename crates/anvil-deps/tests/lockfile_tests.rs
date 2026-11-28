//! Lockfile-resolved auditing: OSV must see the exact locked version, never the manifest requirement.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use anvil_deps::{
    AuditOptions, DepIssueKind, DependencySpec, Ecosystem, PackageHealthInfo, RegistryFetcher,
    audit_manifests,
};
use anyhow::Result;
use chrono::Utc;
use rustc_hash::FxHashMap;
use tempfile::TempDir;

/// `(name, resolved_version, version_req)` as seen by the OSV seam.
type Query = (String, Option<String>, Option<String>);

/// Registry mock that flags every spec whose OSV version is below `fixed_in`, and records the queries.
struct VersionAwareFetcher {
    fixed_in: &'static str,
    queried: Mutex<Vec<Query>>,
}

impl VersionAwareFetcher {
    fn new(fixed_in: &'static str) -> Self {
        Self { fixed_in, queried: Mutex::new(Vec::new()) }
    }

    fn queried(&self) -> Vec<Query> {
        self.queried.lock().expect("lock").clone()
    }
}

fn numeric(version: &str) -> Vec<u32> {
    version.trim_start_matches(['^', '~', '=']).split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

impl RegistryFetcher for VersionAwareFetcher {
    fn fetch(&self, ecosystem: Ecosystem, package: &str) -> Result<PackageHealthInfo> {
        Ok(PackageHealthInfo {
            ecosystem,
            name: package.to_owned(),
            latest_version: Some("9.9.9".to_owned()),
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
            self.queried.lock().expect("lock").push((
                spec.name.clone(),
                spec.resolved_version.clone(),
                spec.version_req.clone(),
            ));
            let effective = spec.resolved_version.as_deref().or(spec.version_req.as_deref());
            if effective.is_some_and(|v| numeric(v) < numeric(self.fixed_in)) {
                out.insert(idx, vec!["ADV-1".to_owned()]);
            }
        }
        out
    }
}

fn write(root: &std::path::Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, content).expect("write");
}

fn audit(
    root: &std::path::Path,
    manifest: &str,
    fetcher: &dyn RegistryFetcher,
) -> anvil_deps::DepsAuditReport {
    audit_manifests(root, &[PathBuf::from(manifest)], fetcher, &AuditOptions::default())
        .expect("audit")
}

#[test]
fn cargo_workspace_inherited_regex_audits_the_locked_version_only() {
    let tmp = TempDir::new().expect("tempdir");
    write(
        tmp.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\nregex = \"1\"\n",
    );
    write(
        tmp.path(),
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nregex.workspace = true\n",
    );
    write(
        tmp.path(),
        "Cargo.lock",
        "[[package]]\nname = \"regex\"\nversion = \"1.11.1\"\nsource = \"registry+x\"\n",
    );

    let fetcher = VersionAwareFetcher::new("1.5.5");
    let root = audit(tmp.path(), "Cargo.toml", &fetcher);
    let member = audit(tmp.path(), "crates/app/Cargo.toml", &fetcher);

    assert!(root.findings.is_empty(), "{:?}", root.findings);
    assert!(member.findings.is_empty(), "{:?}", member.findings);
    assert_eq!(member.checked_packages, 1);
    assert!(fetcher.queried().iter().all(|(_, resolved, _)| resolved.as_deref() == Some("1.11.1")));
}

#[test]
fn cargo_crate_locked_at_two_versions_reports_the_vulnerable_one() {
    let tmp = TempDir::new().expect("tempdir");
    write(tmp.path(), "Cargo.toml", "[dependencies]\nregex = \"1\"\n");
    write(
        tmp.path(),
        "Cargo.lock",
        "[[package]]\nname = \"regex\"\nversion = \"1.4.0\"\nsource = \"registry+x\"\n\n[[package]]\nname = \"regex\"\nversion = \"1.11.1\"\nsource = \"registry+x\"\n",
    );

    let fetcher = VersionAwareFetcher::new("1.5.5");
    let report = audit(tmp.path(), "Cargo.toml", &fetcher);

    assert_eq!(report.checked_packages, 2);
    assert_eq!(report.findings.len(), 1);
    let finding = &report.findings[0];
    assert_eq!(finding.issues, vec![DepIssueKind::Vulnerable]);
    assert_eq!(finding.resolved_version.as_deref(), Some("1.4.0"));
    assert_eq!(finding.label(), "cargo:regex 1.4.0 (Cargo.lock)");
}

#[test]
fn npm_package_lock_v3_resolution_overrides_the_caret_range() {
    let tmp = TempDir::new().expect("tempdir");
    write(tmp.path(), "package.json", r#"{ "dependencies": { "lodash": "^4.0.0" } }"#);
    write(
        tmp.path(),
        "package-lock.json",
        r#"{ "lockfileVersion": 3, "packages": { "": {}, "node_modules/lodash": { "version": "4.17.21" } } }"#,
    );

    let fetcher = VersionAwareFetcher::new("4.17.21");
    let report = audit(tmp.path(), "package.json", &fetcher);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert_eq!(
        fetcher.queried(),
        vec![("lodash".to_owned(), Some("4.17.21".to_owned()), Some("^4.0.0".to_owned()))]
    );
}

#[test]
fn composer_lock_resolution_flags_the_locked_version() {
    let tmp = TempDir::new().expect("tempdir");
    write(tmp.path(), "composer.json", r#"{ "require": { "monolog/monolog": "^3.0" } }"#);
    write(
        tmp.path(),
        "composer.lock",
        r#"{ "packages": [ { "name": "monolog/monolog", "version": "3.0.1" } ] }"#,
    );

    let fetcher = VersionAwareFetcher::new("3.2.0");
    let report = audit(tmp.path(), "composer.json", &fetcher);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].label(), "composer:monolog/monolog 3.0.1 (composer.lock)");
}

#[test]
fn missing_lockfile_marks_the_finding_as_unresolved() {
    let tmp = TempDir::new().expect("tempdir");
    write(tmp.path(), "Cargo.toml", "[dependencies]\nregex = \"1\"\n");

    let fetcher = VersionAwareFetcher::new("1.5.5");
    let report = audit(tmp.path(), "Cargo.toml", &fetcher);

    assert_eq!(report.findings.len(), 1);
    let finding = &report.findings[0];
    assert_eq!(finding.resolved_version, None);
    assert_eq!(finding.lockfile, None);
    assert_eq!(finding.label(), "cargo:regex version=unresolved(req \"1\") (Cargo.toml)");
}

#[test]
fn version_bump_in_the_lockfile_requeries_even_with_a_warm_registry_cache() {
    let tmp = TempDir::new().expect("tempdir");
    write(tmp.path(), "package.json", r#"{ "dependencies": { "lodash": "^4.0.0" } }"#);
    write(
        tmp.path(),
        "package-lock.json",
        r#"{ "lockfileVersion": 3, "packages": { "node_modules/lodash": { "version": "4.17.20" } } }"#,
    );

    let fetcher = VersionAwareFetcher::new("4.17.21");
    let before = audit(tmp.path(), "package.json", &fetcher);
    assert_eq!(before.findings.len(), 1);

    write(
        tmp.path(),
        "package-lock.json",
        r#"{ "lockfileVersion": 3, "packages": { "node_modules/lodash": { "version": "4.17.21" } } }"#,
    );
    let after = audit(tmp.path(), "package.json", &fetcher);
    assert_eq!(after.cache_hits, 1);
    assert!(after.findings.is_empty(), "{:?}", after.findings);
}
