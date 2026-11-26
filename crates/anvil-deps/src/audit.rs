//! Orchestrator for Tier 2 dependency health auditing with TTL cache & bounded parallel fetch.

mod worker;

use std::fs;
use std::path::{Path, PathBuf};

use anvil_core::config::AnvilConfig;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::cache::DepsCache;
use crate::cargo_toml::{fetch_cargo, parse_cargo_toml};
use crate::composer::{fetch_composer, parse_composer_json};
use crate::maven::{
    fetch_maven, parse_build_gradle, parse_build_gradle_kts, parse_libs_versions_toml,
};
use crate::model::{
    DependencySpec, DepsAuditReport, Ecosystem, PackageHealthInfo, RegistryFetcher,
};
use crate::npm::{fetch_npm, parse_package_json};
use crate::pubdev::{fetch_pubdev, parse_pubspec_yaml};
use crate::pypi::{fetch_pypi, parse_pyproject_toml, parse_requirements_txt};
use worker::{evaluate_health, fetch_misses_bounded};

const DEFAULT_MAX_AGE_MONTHS: u32 = 24;
const DEFAULT_CACHE_TTL_HOURS: i64 = 24;

/// Synchronous HTTP fetcher querying npm, PyPI, pub.dev, Maven Central/Google Maven, and crates.io.
#[derive(Debug, Default, Clone, Copy)]
pub struct LiveRegistryFetcher;

impl RegistryFetcher for LiveRegistryFetcher {
    fn fetch(&self, ecosystem: Ecosystem, package: &str) -> Result<PackageHealthInfo> {
        match ecosystem {
            Ecosystem::Npm => fetch_npm(package),
            Ecosystem::Pypi => fetch_pypi(package),
            Ecosystem::PubDev => fetch_pubdev(package),
            Ecosystem::Maven => fetch_maven(package),
            Ecosystem::Cargo => fetch_cargo(package),
            Ecosystem::Composer => fetch_composer(package),
        }
    }

    fn fetch_vulnerabilities(&self, specs: &[DependencySpec]) -> FxHashMap<usize, Vec<String>> {
        crate::osv::query_osv_batch(specs)
    }
}

const fn default_true() -> bool {
    true
}

/// Configurable thresholds and clock override for dependency audits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditOptions {
    pub max_age_months: u32,
    pub cache_ttl_hours: i64,
    pub now: Option<DateTime<Utc>>,
    #[serde(default = "default_true")]
    pub check_osv: bool,
}

impl Default for AuditOptions {
    fn default() -> Self {
        Self {
            max_age_months: DEFAULT_MAX_AGE_MONTHS,
            cache_ttl_hours: DEFAULT_CACHE_TTL_HOURS,
            now: None,
            check_osv: true,
        }
    }
}

impl AuditOptions {
    /// Loads `deps_max_age_months` from `.harness/config.json` (falling back to 24 months).
    pub fn from_repo(repo_root: &Path) -> Result<Self> {
        let cfg = AnvilConfig::load_or_default(repo_root)?;
        Ok(Self { max_age_months: cfg.deps_max_age_months, ..Self::default() })
    }
}

fn is_requirements_txt_name(name: &str) -> bool {
    name == "requirements.txt"
        || ((name.starts_with("requirements-") || name.starts_with("requirements_"))
            && name.ends_with(".txt"))
}

/// Returns true when `path` is a supported dependency manifest filename.
pub fn is_supported_manifest(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|name| {
        matches!(
            name,
            "package.json"
                | "pyproject.toml"
                | "pubspec.yaml"
                | "build.gradle.kts"
                | "build.gradle"
                | "libs.versions.toml"
                | "Cargo.toml"
                | "composer.json"
        ) || is_requirements_txt_name(name)
    })
}

/// Parses a single manifest file into ecosystem dependency specs.
pub fn parse_manifest(repo_root: &Path, path: &Path) -> Result<Vec<DependencySpec>> {
    let abs = if path.is_absolute() { path.to_path_buf() } else { repo_root.join(path) };
    let rel = abs.strip_prefix(repo_root).unwrap_or(&abs).to_string_lossy().replace('\\', "/");
    let content = fs::read_to_string(&abs)
        .with_context(|| format!("failed to read manifest {}", abs.display()))?;

    match abs.file_name().and_then(|n| n.to_str()) {
        Some("package.json") => parse_package_json(&rel, &content),
        Some("pyproject.toml") => parse_pyproject_toml(&rel, &content),
        Some(name) if is_requirements_txt_name(name) => parse_requirements_txt(&rel, &content),
        Some("pubspec.yaml") => parse_pubspec_yaml(&rel, &content),
        Some("build.gradle.kts") => parse_build_gradle_kts(&rel, &content),
        Some("build.gradle") => parse_build_gradle(&rel, &content),
        Some("libs.versions.toml") => parse_libs_versions_toml(&rel, &content),
        Some("Cargo.toml") => parse_cargo_toml(&rel, &content),
        Some("composer.json") => parse_composer_json(&rel, &content),
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
    let manifests: Vec<PathBuf> =
        changed_paths.iter().filter(|p| is_supported_manifest(p)).cloned().collect();
    audit_manifests(repo_root, &manifests, fetcher, options)
}

/// Audits the specified manifest files using `.harness/.state/deps-cache.json` + bounded parallel fetches.
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

    let mut resolved: Vec<Option<PackageHealthInfo>> = vec![None; specs.len()];
    let mut misses: Vec<(usize, &DependencySpec)> = Vec::new();
    for (idx, spec) in specs.iter().enumerate() {
        if let Some(cached) =
            cache.get_fresh(spec.ecosystem, &spec.name, options.cache_ttl_hours, now)
        {
            report.cache_hits += 1;
            resolved[idx] = Some(cached.clone());
        } else {
            misses.push((idx, spec));
        }
    }

    apply_fetched_misses(&specs, &misses, fetcher, now, &mut cache, &mut resolved, &mut report);

    if options.check_osv {
        merge_osv_vulnerabilities(&specs, &mut resolved, fetcher);
    }

    for (spec, maybe_info) in specs.iter().zip(resolved) {
        if let Some(info) = maybe_info
            && let Some(finding) = evaluate_health(&info, spec, options.max_age_months, now)
        {
            report.findings.push(finding);
        }
    }

    cache.prune_expired(options.cache_ttl_hours, now);
    cache.save_if_dirty()?;
    Ok(report)
}

fn apply_fetched_misses(
    specs: &[DependencySpec],
    misses: &[(usize, &DependencySpec)],
    fetcher: &dyn RegistryFetcher,
    now: DateTime<Utc>,
    cache: &mut DepsCache,
    resolved: &mut [Option<PackageHealthInfo>],
    report: &mut DepsAuditReport,
) {
    let mut grouped: FxHashMap<(Ecosystem, String), Vec<usize>> = FxHashMap::default();
    let mut unique_misses: Vec<(usize, &DependencySpec)> = Vec::new();
    for &(idx, spec) in misses {
        let key = (spec.ecosystem, spec.name.clone());
        let indices = grouped.entry(key).or_default();
        if indices.is_empty() {
            unique_misses.push((idx, spec));
        }
        indices.push(idx);
    }

    for (idx, result) in fetch_misses_bounded(&unique_misses, fetcher) {
        let spec = &specs[idx];
        match result {
            Ok(fetched) => {
                report.network_fetches += 1;
                cache.insert(fetched.clone(), now);
                if let Some(indices) = grouped.get(&(spec.ecosystem, spec.name.clone())) {
                    for &target_idx in indices {
                        resolved[target_idx] = Some(fetched.clone());
                    }
                }
            }
            Err(err) => {
                report.errors.push(format!("{}:{}: {err:#}", spec.ecosystem, spec.name));
            }
        }
    }
}

fn merge_osv_vulnerabilities(
    specs: &[DependencySpec],
    resolved: &mut [Option<PackageHealthInfo>],
    fetcher: &dyn RegistryFetcher,
) {
    let vulns_map = fetcher.fetch_vulnerabilities(specs);
    for (idx, maybe_info) in resolved.iter_mut().enumerate() {
        if let Some(vulns) = vulns_map.get(&idx)
            && let Some(info) = maybe_info.as_mut()
        {
            for vuln in vulns {
                if !info.vulnerabilities.contains(vuln) {
                    info.vulnerabilities.push(vuln.clone());
                }
            }
        }
    }
}

fn collect_unique_specs(repo_root: &Path, manifests: &[PathBuf]) -> Result<Vec<DependencySpec>> {
    let mut seen = FxHashSet::default();
    let mut specs = Vec::new();
    for manifest in manifests {
        for spec in parse_manifest(repo_root, manifest)? {
            if seen.insert((spec.ecosystem, spec.name.clone(), spec.version_req.clone())) {
                specs.push(spec);
            }
        }
    }
    Ok(specs)
}
