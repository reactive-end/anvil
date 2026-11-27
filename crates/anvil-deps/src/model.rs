//! Domain types and `RegistryFetcher` trait for `anvil-deps`.

use std::fmt;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Supported package ecosystems for Tier 2 dependency health auditing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ecosystem {
    Npm,
    Pypi,
    PubDev,
    Maven,
    Cargo,
    Composer,
}

impl Ecosystem {
    /// Returns the canonical lowercase identifier for cache keys and display.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pypi => "pypi",
            Self::PubDev => "pubdev",
            Self::Maven => "maven",
            Self::Cargo => "cargo",
            Self::Composer => "composer",
        }
    }
}

impl fmt::Display for Ecosystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Dependency entry extracted from a manifest file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencySpec {
    pub ecosystem: Ecosystem,
    pub name: String,
    pub version_req: Option<String>,
    pub manifest_path: String,
    /// Exact version pinned by the lockfile next to the manifest, when one was found.
    #[serde(default)]
    pub resolved_version: Option<String>,
    /// Repo-relative path of the lockfile `resolved_version` came from.
    #[serde(default)]
    pub lockfile: Option<String>,
}

/// Describes how the audited version was determined (exact lock version vs unresolved requirement).
fn version_text(resolved: Option<&str>, req: Option<&str>) -> String {
    match (resolved, req) {
        (Some(version), _) => version.to_owned(),
        (None, Some(req)) => format!("version=unresolved(req \"{req}\")"),
        (None, None) => "version=unresolved".to_owned(),
    }
}

/// Metadata retrieved from a package registry (or local TTL cache).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageHealthInfo {
    pub ecosystem: Ecosystem,
    pub name: String,
    pub latest_version: Option<String>,
    pub last_publish: Option<DateTime<Utc>>,
    pub deprecated: bool,
    pub deprecation_message: Option<String>,
    pub archived: bool,
    pub repository_url: Option<String>,
    #[serde(default)]
    pub vulnerabilities: Vec<String>,
    /// Informational advisories (`unmaintained`, `unsound`, `notice`): warnings, not vulnerabilities.
    #[serde(default)]
    pub informational: Vec<String>,
}

/// Health risk classification for a dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepIssueKind {
    Deprecated,
    Archived,
    Stale,
    Vulnerable,
    /// Informational advisory (RustSec `unmaintained`/`unsound`/`notice`); warns but never blocks.
    Informational,
}

/// Actionable finding produced when a dependency violates health policies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepFinding {
    pub ecosystem: Ecosystem,
    pub package: String,
    pub manifest_path: String,
    pub issues: Vec<DepIssueKind>,
    pub age_months: Option<u32>,
    pub last_publish: Option<DateTime<Utc>>,
    pub deprecation_message: Option<String>,
    pub latest_version: Option<String>,
    #[serde(default)]
    pub vulnerabilities: Vec<String>,
    /// Informational advisory ids (unmaintained/unsound/notice).
    #[serde(default)]
    pub informational: Vec<String>,
    /// Version audited against OSV: the lockfile-resolved one, or `None` when only a requirement exists.
    #[serde(default)]
    pub resolved_version: Option<String>,
    /// Lockfile (repo-relative) that pinned `resolved_version`.
    #[serde(default)]
    pub lockfile: Option<String>,
    /// Manifest requirement, kept so unresolved findings can explain themselves.
    #[serde(default)]
    pub version_req: Option<String>,
}

impl DepFinding {
    /// True unless every issue is informational; only blocking findings fail Tier 2.
    pub fn is_blocking(&self) -> bool {
        self.issues.iter().any(|kind| *kind != DepIssueKind::Informational)
    }

    /// Display label such as `cargo:regex 1.12.2 (Cargo.lock)` or
    /// `cargo:regex version=unresolved(req "1") (Cargo.toml)`.
    pub fn label(&self) -> String {
        let version = version_text(self.resolved_version.as_deref(), self.version_req.as_deref());
        let source = self.lockfile.as_deref().unwrap_or(&self.manifest_path);
        format!("{}:{} {version} ({source})", self.ecosystem, self.package)
    }
}

/// Summary report returned by `anvil-deps` audits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepsAuditReport {
    pub checked_packages: usize,
    pub cache_hits: usize,
    pub network_fetches: usize,
    pub max_age_months: u32,
    pub findings: Vec<DepFinding>,
    pub errors: Vec<String>,
}

impl DepsAuditReport {
    /// Number of findings that fail Tier 2 (informational-only findings excluded).
    pub fn blocking_count(&self) -> usize {
        self.findings.iter().filter(|f| f.is_blocking()).count()
    }
}

/// Abstracts registry HTTP lookups so tests can inject deterministic offline mocks.
pub trait RegistryFetcher: Send + Sync {
    fn fetch(&self, ecosystem: Ecosystem, package: &str) -> Result<PackageHealthInfo>;

    fn fetch_vulnerabilities(
        &self,
        _specs: &[DependencySpec],
    ) -> rustc_hash::FxHashMap<usize, Vec<String>> {
        rustc_hash::FxHashMap::default()
    }

    /// Returns the subset of advisory `ids` that are informational (unmaintained/unsound/notice).
    fn fetch_informational_advisories(&self, _ids: &[String]) -> rustc_hash::FxHashSet<String> {
        rustc_hash::FxHashSet::default()
    }
}
