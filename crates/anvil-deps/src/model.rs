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
}

impl Ecosystem {
    /// Returns the canonical lowercase identifier for cache keys and display.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pypi => "pypi",
            Self::PubDev => "pubdev",
            Self::Maven => "maven",
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
}

/// Health risk classification for a dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepIssueKind {
    Deprecated,
    Archived,
    Stale,
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

/// Abstracts registry HTTP lookups so tests can inject deterministic offline mocks.
pub trait RegistryFetcher: Send + Sync {
    fn fetch(&self, ecosystem: Ecosystem, package: &str) -> Result<PackageHealthInfo>;
}
