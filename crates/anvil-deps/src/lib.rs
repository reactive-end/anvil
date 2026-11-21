//! Tier 2 dependency health auditing across npm, PyPI, pub.dev, and Maven.

pub mod audit;
pub mod maven;
pub mod model;
pub mod npm;
pub mod pubdev;
pub mod pypi;

mod cache;
mod http;

pub use audit::{
    AuditOptions, LiveRegistryFetcher, audit_changed_files, audit_manifests, is_supported_manifest,
    parse_manifest,
};
pub use model::{
    DepFinding, DepIssueKind, DependencySpec, DepsAuditReport, Ecosystem, PackageHealthInfo,
    RegistryFetcher,
};
