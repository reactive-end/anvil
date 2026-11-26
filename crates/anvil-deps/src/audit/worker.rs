//! Bounded parallel registry fetching and health policy evaluation for `anvil-deps`.

use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::model::{
    DepFinding, DepIssueKind, DependencySpec, Ecosystem, PackageHealthInfo, RegistryFetcher,
};

const MAX_CONCURRENT_FETCHES: usize = 8;

/// Fetches uncached specs, running `Cargo` misses sequentially (crates.io `<= 1 req/s` policy)
/// while keeping up to 8 concurrent worker threads for npm/PyPI/pub.dev/Maven.
pub(crate) fn fetch_misses_bounded(
    misses: &[(usize, &DependencySpec)],
    fetcher: &dyn RegistryFetcher,
) -> Vec<(usize, Result<PackageHealthInfo>)> {
    if misses.is_empty() {
        return Vec::new();
    }

    let (cargo_misses, parallel_misses): (Vec<_>, Vec<_>) =
        misses.iter().copied().partition(|(_, spec)| spec.ecosystem == Ecosystem::Cargo);

    let mut out = Vec::with_capacity(misses.len());
    std::thread::scope(|scope| {
        let cargo_handle = (!cargo_misses.is_empty()).then(|| {
            scope.spawn(|| {
                cargo_misses
                    .iter()
                    .map(|&(idx, spec)| (idx, fetcher.fetch(spec.ecosystem, &spec.name)))
                    .collect::<Vec<_>>()
            })
        });

        out.extend(fetch_parallel_misses(&parallel_misses, fetcher));

        if let Some(handle) = cargo_handle
            && let Ok(part) = handle.join()
        {
            out.extend(part);
        }
    });

    out.sort_by_key(|(spec_idx, _)| *spec_idx);
    out
}

fn fetch_parallel_misses(
    misses: &[(usize, &DependencySpec)],
    fetcher: &dyn RegistryFetcher,
) -> Vec<(usize, Result<PackageHealthInfo>)> {
    if misses.is_empty() {
        return Vec::new();
    }
    if misses.len() == 1 {
        let (idx, spec) = misses[0];
        return vec![(idx, fetcher.fetch(spec.ecosystem, &spec.name))];
    }

    let next = AtomicUsize::new(0);
    let worker_count = misses.len().min(MAX_CONCURRENT_FETCHES);
    let mut out = Vec::with_capacity(misses.len());

    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..worker_count)
            .map(|_| {
                scope.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        let pos = next.fetch_add(1, Ordering::Relaxed);
                        if pos >= misses.len() {
                            break;
                        }
                        let (spec_idx, spec) = misses[pos];
                        local.push((spec_idx, fetcher.fetch(spec.ecosystem, &spec.name)));
                    }
                    local
                })
            })
            .collect();

        for handle in handles {
            if let Ok(part) = handle.join() {
                out.extend(part);
            }
        }
    });

    out
}

/// Evaluates deprecation, archival, and staleness thresholds for a resolved package.
pub(crate) fn evaluate_health(
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
    if !info.vulnerabilities.is_empty() {
        issues.push(DepIssueKind::Vulnerable);
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
        vulnerabilities: info.vulnerabilities.clone(),
    })
}
