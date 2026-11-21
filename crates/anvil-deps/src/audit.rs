//! Orchestrator for Tier 2 dependency health auditing with local TTL disk caching.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use anvil_core::config::AnvilConfig;
use chrono::{DateTime, Utc};
use rustc_hash::FxHashSet;

use crate::cache::DepsCache;
use crate::maven::{fetch_maven, parse_build_gradle_kts, parse_libs_versions_toml};
use crate::model::{
    DepFinding, DepIssueKind, DependencySpec, DepsAuditReport, Ecosystem, PackageHealthInfo,
    RegistryFetcher,
};
use crate::npm::{fetch_npm, parse_package_json};
use crate::pubdev::{fetch_pubdev, parse_pubspec_yaml};
use crate::pypi::{fetch_pypi, parse_pyproject_toml};

const DEFAULT_MAX_AGE_MONTHS: u32 = 24;
const DEFAULT_CACHE_TTL_HOURS: i64 = 24;

/// Synchronous HTTP fetcher querying live npm, PyPI, pub.dev, and Maven Central APIs.
#[derive(Debug, Default, Clone, Copy)]
pub struct LiveRegistryFetcher;

impl RegistryFetcher for LiveRegistryFetcher {
    fn fetch(&self, ecosystem: Ecosystem, package: &str) -> Result<PackageHealthInfo> {
        match ecosystem {
            Ecosystem::Npm => fetch_npm(package),
            Ecosystem::Pypi => fetch_pypi(package),
            Ecosystem::PubDev => fetch_pubdev(package),
            Ecosystem::Maven => fetch_maven(package),
        }
    }
}

/// Configurable thresholds and clock override for dependency audits.
#[derive(Debug, Clone)]
pub struct AuditOptions {
    pub max_age_months: u32,
    pub cache_ttl_hours: i64,
    pub now: Option<DateTime<Utc>>,
}

impl Default for AuditOptions {
    fn default() -> Self {
        Self {
            max_age_months: DEFAULT_MAX_AGE_MONTHS,
            cache_ttl_hours: DEFAULT_CACHE_TTL_HOURS,
            now: None,
        }
    }
}

impl AuditOptions {
    /// Loads `deps_max_age_months` from `.harness/config.json` (falling back to 24 months).
    pub fn from_repo(repo_root: &Path) -> Result<Self> {
        let cfg = AnvilConfig::load_or_default(repo_root)?;
        Ok(Self {
            max_age_months: cfg.deps_max_age_months,
            ..Self::default()
        })
    }
}

/// Returns true when `path` is a supported dependency manifest filename.
pub fn is_supported_manifest(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some(
            "package.json"
                | "pyproject.toml"
                | "pubspec.yaml"
                | "build.gradle.kts"
                | "libs.versions.toml"
        )
    )
}

/// Parses a single manifest file into ecosystem dependency specs.
pub fn parse_manifest(repo_root: &Path, path: &Path) -> Result<Vec<DependencySpec>> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_root.join(path)
    };
    let rel = abs
        .strip_prefix(repo_root)
        .unwrap_or(&abs)
        .to_string_lossy()
        .replace('\\', "/");
    let content = fs::read_to_string(&abs)
        .with_context(|| format!("failed to read manifest {}", abs.display()))?;

    match abs.file_name().and_then(|n| n.to_str()) {
        Some("package.json") => parse_package_json(&rel, &content),
        Some("pyproject.toml") => parse_pyproject_toml(&rel, &content),
        Some("pubspec.yaml") => parse_pubspec_yaml(&rel, &content),
        Some("build.gradle.kts") => parse_build_gradle_kts(&rel, &content),
        Some("libs.versions.toml") => parse_libs_versions_toml(&rel, &content),
        _ => bail!("unsupported dependency manifest: {}", abs.display()),
    }
}

/// Audits only the supported dependency manifests inside `changed_paths`.
pub fn audit_changed_files(
    repo_root: &Path,
    changed_paths: &[PathBuf],
    fetcher: &dyn RegistryFetcher,
    options: &AuditOptions,
) -> Result<DepsAuditReport> {
    let manifests: Vec<PathBuf> = changed_paths
        .iter()
        .filter(|p| is_supported_manifest(p))
        .cloned()
        .collect();
    audit_manifests(repo_root, &manifests, fetcher, options)
}

/// Audits the specified manifest files using `.harness/.state/deps-cache.json` + `fetcher`.
pub fn audit_manifests(
    repo_root: &Path,
    manifests: &[PathBuf],
    fetcher: &dyn RegistryFetcher,
    options: &AuditOptions,
) -> Result<DepsAuditReport> {
    let now = options.now.unwrap_or_else(Utc::now);
    let specs = collect_unique_specs(repo_root, manifests)?;
    let mut cache = DepsCache::load(repo_root);

    let mut report = DepsAuditReport {
        checked_packages: specs.len(),
        cache_hits: 0,
        network_fetches: 0,
        max_age_months: options.max_age_months,
        findings: Vec::new(),
        errors: Vec::new(),
    };

    for spec in specs {
        audit_single_spec(spec, &mut cache, fetcher, options, now, &mut report);
    }

    cache.save_if_dirty()?;
    Ok(report)
}

fn collect_unique_specs(repo_root: &Path, manifests: &[PathBuf]) -> Result<Vec<DependencySpec>> {
    let mut seen = FxHashSet::default();
    let mut specs = Vec::new();
    for manifest in manifests {
        for spec in parse_manifest(repo_root, manifest)? {
            if seen.insert((spec.ecosystem, spec.name.clone())) {
                specs.push(spec);
            }
        }
    }
    Ok(specs)
}

fn audit_single_spec(
    spec: DependencySpec,
    cache: &mut DepsCache,
    fetcher: &dyn RegistryFetcher,
    options: &AuditOptions,
    now: DateTime<Utc>,
    report: &mut DepsAuditReport,
) {
    let info = if let Some(cached) =
        cache.get_fresh(spec.ecosystem, &spec.name, options.cache_ttl_hours, now)
    {
        report.cache_hits += 1;
        cached.clone()
    } else {
        match fetcher.fetch(spec.ecosystem, &spec.name) {
            Ok(fetched) => {
                report.network_fetches += 1;
                cache.insert(fetched.clone(), now);
                fetched
            }
            Err(err) => {
                report
                    .errors
                    .push(format!("{}:{}: {err:#}", spec.ecosystem, spec.name));
                return;
            }
        }
    };

    if let Some(finding) = evaluate_health(&info, &spec, options.max_age_months, now) {
        report.findings.push(finding);
    }
}

fn evaluate_health(
    info: &PackageHealthInfo,
    spec: &DependencySpec,
    max_age_months: u32,
    now: DateTime<Utc>,
) -> Option<DepFinding> {
    let mut issues = Vec::new();
    if info.deprecated {
        issues.push(DepIssueKind::Deprecated);
    }
    if info.archived {
        issues.push(DepIssueKind::Archived);
    }

    let age_months = info.last_publish.map(|published| {
        let days = (now - published).num_days().max(0);
        ((days * 12) / 365) as u32
    });
    if age_months.is_some_and(|months| months > max_age_months) {
        issues.push(DepIssueKind::Stale);
    }

    (!issues.is_empty()).then(|| DepFinding {
        ecosystem: spec.ecosystem,
        package: spec.name.clone(),
        manifest_path: spec.manifest_path.clone(),
        issues,
        age_months,
        last_publish: info.last_publish,
        deprecation_message: info.deprecation_message.clone(),
        latest_version: info.latest_version.clone(),
    })
}
