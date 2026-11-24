//! Offline orchestrator and TTL cache tests using an injectable `RegistryFetcher`.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration as StdDuration, Instant};

use anvil_deps::{
    AuditOptions, DepIssueKind, Ecosystem, PackageHealthInfo, RegistryFetcher, audit_changed_files,
    audit_manifests,
};
use anyhow::{Result, bail};
use chrono::{DateTime, Duration, Utc};
use rustc_hash::FxHashMap;
use tempfile::TempDir;

struct MockFetcher {
    entries: FxHashMap<(Ecosystem, String), PackageHealthInfo>,
    calls: AtomicUsize,
}

impl MockFetcher {
    fn new(items: Vec<PackageHealthInfo>) -> Self {
        let mut entries = FxHashMap::default();
        for item in items {
            entries.insert((item.ecosystem, item.name.clone()), item);
        }
        Self {
            entries,
            calls: AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

impl RegistryFetcher for MockFetcher {
    fn fetch(&self, ecosystem: Ecosystem, package: &str) -> Result<PackageHealthInfo> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        match self.entries.get(&(ecosystem, package.to_owned())) {
            Some(info) => Ok(info.clone()),
            None => bail!("mock package not found: {ecosystem}:{package}"),
        }
    }
}

fn mk_pkg(
    ecosystem: Ecosystem,
    name: &str,
    last_publish: DateTime<Utc>,
    deprecated: bool,
    archived: bool,
) -> PackageHealthInfo {
    PackageHealthInfo {
        ecosystem,
        name: name.to_owned(),
        latest_version: Some("1.0.0".to_owned()),
        last_publish: Some(last_publish),
        deprecated,
        deprecation_message: deprecated.then(|| "use replacement".to_owned()),
        archived,
        repository_url: None,
        vulnerabilities: Vec::new(),
    }
}

#[test]
fn flags_deprecated_archived_and_stale_packages_while_passing_fresh() {
    let tmp = TempDir::new().expect("tempdir");
    let now = Utc::now();
    let fresh_ts = now - Duration::days(30);
    let stale_ts = now - Duration::days(365 * 3);

    fs::write(
        tmp.path().join("package.json"),
        r#"{ "dependencies": { "fresh-pkg": "^1.0.0", "old-pkg": "^1.0.0", "dead-pkg": "^1.0.0" } }"#,
    )
    .expect("write package.json");
    fs::write(
        tmp.path().join("pyproject.toml"),
        "[project]\ndependencies = [\"archived-py>=1.0\"]\n",
    )
    .expect("write pyproject.toml");

    let fetcher = MockFetcher::new(vec![
        mk_pkg(Ecosystem::Npm, "fresh-pkg", fresh_ts, false, false),
        mk_pkg(Ecosystem::Npm, "old-pkg", stale_ts, false, false),
        mk_pkg(Ecosystem::Npm, "dead-pkg", fresh_ts, true, false),
        mk_pkg(Ecosystem::Pypi, "archived-py", fresh_ts, false, true),
    ]);

    let opts = AuditOptions {
        max_age_months: 24,
        cache_ttl_hours: 24,
        now: Some(now),
        check_osv: true,
    };
    let manifests = vec![PathBuf::from("package.json"), PathBuf::from("pyproject.toml")];
    let report = audit_manifests(tmp.path(), &manifests, &fetcher, &opts).expect("audit");

    assert_eq!(report.checked_packages, 4);
    assert_eq!(report.network_fetches, 4);
    assert_eq!(report.cache_hits, 0);
    assert!(report.errors.is_empty());
    assert_eq!(report.findings.len(), 3);

    let find = |pkg: &str| report.findings.iter().find(|f| f.package == pkg).expect("finding");
    assert_eq!(find("dead-pkg").issues, vec![DepIssueKind::Deprecated]);
    assert_eq!(find("old-pkg").issues, vec![DepIssueKind::Stale]);
    assert_eq!(find("archived-py").issues, vec![DepIssueKind::Archived]);
    assert!(report.findings.iter().all(|f| f.package != "fresh-pkg"));
}

#[test]
fn repeat_audit_hits_disk_cache_under_two_milliseconds() {
    let tmp = TempDir::new().expect("tempdir");
    let now = Utc::now();
    let fresh_ts = now - Duration::days(15);

    fs::write(
        tmp.path().join("pubspec.yaml"),
        "dependencies:\n  dio: ^5.0.0\n  intl: ^0.19.0\n",
    )
    .expect("write pubspec.yaml");

    let fetcher = MockFetcher::new(vec![
        mk_pkg(Ecosystem::PubDev, "dio", fresh_ts, false, false),
        mk_pkg(Ecosystem::PubDev, "intl", fresh_ts, false, false),
    ]);
    let opts = AuditOptions {
        max_age_months: 24,
        cache_ttl_hours: 24,
        now: Some(now),
        check_osv: true,
    };
    let changed = vec![PathBuf::from("src/main.dart"), PathBuf::from("pubspec.yaml")];

    let first = audit_changed_files(tmp.path(), &changed, &fetcher, &opts).expect("first");
    assert_eq!(first.checked_packages, 2);
    assert_eq!(first.network_fetches, 2);
    assert_eq!(fetcher.call_count(), 2);
    assert!(tmp.path().join(".harness/.state/deps-cache.json").exists());

    // Second run warms OS/AV file cache; third run verifies steady-state disk cache hit < 2ms.
    let second = audit_changed_files(tmp.path(), &changed, &fetcher, &opts).expect("second");
    assert_eq!(second.cache_hits, 2);
    assert_eq!(second.network_fetches, 0);

    let start = Instant::now();
    let third = audit_changed_files(tmp.path(), &changed, &fetcher, &opts).expect("third");
    let elapsed = start.elapsed();

    assert_eq!(third.checked_packages, 2);
    assert_eq!(third.cache_hits, 2);
    assert_eq!(third.network_fetches, 0);
    assert_eq!(fetcher.call_count(), 2);
    assert!(
        elapsed.as_micros() < 2_000,
        "expected cache hit < 2ms, took {}us",
        elapsed.as_micros()
    );
}

#[test]
fn expired_cache_entry_triggers_fresh_fetch() {
    let tmp = TempDir::new().expect("tempdir");
    let t0 = Utc::now();
    fs::write(tmp.path().join("package.json"), r#"{ "dependencies": { "react": "^19.0.0" } }"#)
        .expect("write package.json");

    let fetcher = MockFetcher::new(vec![mk_pkg(
        Ecosystem::Npm,
        "react",
        t0 - Duration::days(10),
        false,
        false,
    )]);
    let manifests = vec![PathBuf::from("package.json")];

    let first_opts = AuditOptions {
        max_age_months: 24,
        cache_ttl_hours: 24,
        now: Some(t0),
        check_osv: true,
    };
    let r1 = audit_manifests(tmp.path(), &manifests, &fetcher, &first_opts).expect("r1");
    assert_eq!(r1.network_fetches, 1);

    let expired_opts = AuditOptions {
        max_age_months: 24,
        cache_ttl_hours: 24,
        now: Some(t0 + Duration::hours(25)),
        check_osv: true,
    };
    let r2 = audit_manifests(tmp.path(), &manifests, &fetcher, &expired_opts).expect("r2");
    assert_eq!(r2.network_fetches, 1);
    assert_eq!(r2.cache_hits, 0);
    assert_eq!(fetcher.call_count(), 2);
}

struct EcosystemConcurrencyFetcher {
    cargo_in_flight: AtomicUsize,
    cargo_max_in_flight: AtomicUsize,
    npm_in_flight: AtomicUsize,
    npm_max_in_flight: AtomicUsize,
}

impl EcosystemConcurrencyFetcher {
    fn new() -> Self {
        Self {
            cargo_in_flight: AtomicUsize::new(0),
            cargo_max_in_flight: AtomicUsize::new(0),
            npm_in_flight: AtomicUsize::new(0),
            npm_max_in_flight: AtomicUsize::new(0),
        }
    }
}

impl RegistryFetcher for EcosystemConcurrencyFetcher {
    fn fetch(&self, ecosystem: Ecosystem, package: &str) -> Result<PackageHealthInfo> {
        match ecosystem {
            Ecosystem::Cargo => {
                let cur = self.cargo_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                self.cargo_max_in_flight.fetch_max(cur, Ordering::SeqCst);
                thread::sleep(StdDuration::from_millis(15));
                self.cargo_in_flight.fetch_sub(1, Ordering::SeqCst);
            }
            Ecosystem::Npm => {
                let cur = self.npm_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                self.npm_max_in_flight.fetch_max(cur, Ordering::SeqCst);
                thread::sleep(StdDuration::from_millis(30));
                self.npm_in_flight.fetch_sub(1, Ordering::SeqCst);
            }
            _ => {}
        }
        Ok(PackageHealthInfo {
            ecosystem,
            name: package.to_owned(),
            latest_version: Some("1.0.0".to_owned()),
            last_publish: Some(Utc::now() - Duration::days(5)),
            deprecated: false,
            deprecation_message: None,
            archived: false,
            repository_url: None,
            vulnerabilities: Vec::new(),
        })
    }
}

#[test]
fn cargo_misses_run_sequentially_while_npm_keeps_parallel_concurrency() {
    let tmp = TempDir::new().expect("tempdir");
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\na = \"1\"\nb = \"1\"\nc = \"1\"\nd = \"1\"\n",
    )
    .expect("write Cargo.toml");
    fs::write(
        tmp.path().join("package.json"),
        r#"{ "dependencies": { "p1": "1.0.0", "p2": "1.0.0", "p3": "1.0.0", "p4": "1.0.0", "p5": "1.0.0", "p6": "1.0.0" } }"#,
    )
    .expect("write package.json");

    let fetcher = EcosystemConcurrencyFetcher::new();
    let report = audit_manifests(
        tmp.path(),
        &[PathBuf::from("Cargo.toml"), PathBuf::from("package.json")],
        &fetcher,
        &AuditOptions::default(),
    )
    .expect("mixed audit");

    assert_eq!(report.checked_packages, 10);
    assert_eq!(fetcher.cargo_max_in_flight.load(Ordering::SeqCst), 1);
    assert!(fetcher.npm_max_in_flight.load(Ordering::SeqCst) >= 2);
}

struct VulnMockFetcher {
    base: MockFetcher,
    vuln_calls: AtomicUsize,
}

impl VulnMockFetcher {
    fn new(items: Vec<PackageHealthInfo>) -> Self {
        Self {
            base: MockFetcher::new(items),
            vuln_calls: AtomicUsize::new(0),
        }
    }
}

impl RegistryFetcher for VulnMockFetcher {
    fn fetch(&self, ecosystem: Ecosystem, package: &str) -> Result<PackageHealthInfo> {
        self.base.fetch(ecosystem, package)
    }

    fn fetch_vulnerabilities(
        &self,
        specs: &[anvil_deps::DependencySpec],
    ) -> FxHashMap<usize, Vec<String>> {
        self.vuln_calls.fetch_add(1, Ordering::Relaxed);
        let mut map = FxHashMap::default();
        for (idx, spec) in specs.iter().enumerate() {
            if spec.name == "lodash" {
                map.insert(
                    idx,
                    vec![
                        "GHSA-35jh-r3h4-6jhm".to_owned(),
                        "CVE-2021-23337".to_owned(),
                        "GHSA-35jh-r3h4-6jhm".to_owned(),
                    ],
                );
            }
        }
        map
    }
}

#[test]
fn audit_manifests_emits_vulnerable_issue_for_uncached_and_cached_packages() {
    let tmp = TempDir::new().expect("tempdir");
    let now = Utc::now();
    let fresh_ts = now - Duration::days(10);

    fs::write(
        tmp.path().join("package.json"),
        r#"{ "dependencies": { "lodash": "^4.17.20", "safe-pkg": "^1.0.0" } }"#,
    )
    .expect("write package.json");

    let fetcher = VulnMockFetcher::new(vec![
        mk_pkg(Ecosystem::Npm, "lodash", fresh_ts, false, false),
        mk_pkg(Ecosystem::Npm, "safe-pkg", fresh_ts, false, false),
    ]);
    let manifests = vec![PathBuf::from("package.json")];

    // First run with check_osv=false caches packages without vulnerabilities.
    let no_osv_opts = AuditOptions {
        max_age_months: 24,
        cache_ttl_hours: 24,
        now: Some(now),
        check_osv: false,
    };
    let first = audit_manifests(tmp.path(), &manifests, &fetcher, &no_osv_opts).expect("first");
    assert!(first.findings.is_empty());
    assert_eq!(fetcher.vuln_calls.load(Ordering::Relaxed), 0);

    // Second run hits cache and enables check_osv=true; must still emit Vulnerable finding.
    let osv_opts = AuditOptions {
        check_osv: true,
        ..no_osv_opts
    };
    let second = audit_manifests(tmp.path(), &manifests, &fetcher, &osv_opts).expect("second");
    assert_eq!(second.cache_hits, 2);
    assert_eq!(second.network_fetches, 0);
    assert_eq!(fetcher.vuln_calls.load(Ordering::Relaxed), 1);
    assert_eq!(second.findings.len(), 1);
    let finding = &second.findings[0];
    assert_eq!(finding.package, "lodash");
    assert_eq!(finding.issues, vec![DepIssueKind::Vulnerable]);
    assert_eq!(
        finding.vulnerabilities,
        vec![
            "GHSA-35jh-r3h4-6jhm".to_owned(),
            "CVE-2021-23337".to_owned()
        ]
    );
}
