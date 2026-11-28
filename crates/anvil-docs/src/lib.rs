//! Living project documentation in the Open Knowledge Format (OKF v0.3).
//!
//! A bundle is a directory of Markdown concepts with YAML frontmatter under
//! `.harness/docs/<project>/`. This crate discovers bundles, parses and validates concepts, detects
//! documentation that drifted from the code it describes, searches it, and writes the generated parts.

pub mod bundle;
pub mod concept;
pub mod create;
pub mod drift;
pub mod frontmatter;
pub mod git;
pub mod links;
pub mod markdown_edit;
pub mod meta;
pub mod outline;
pub mod paths;
pub mod refresh;
pub mod repo_files;
pub mod scaffold;
pub mod search;
pub mod trust;
pub mod validate;
pub mod yaml;

/// OKF version written to every bundle root `index.md`.
pub const OKF_VERSION: &str = "0.3";

/// Frontmatter key listing the repo-relative globs of the code a concept documents.
pub const KEY_PATHS: &str = "x-anvil-paths";
/// Frontmatter key holding the git commit a concept was last synced to.
pub const KEY_SYNCED_COMMIT: &str = "x-anvil-synced-commit";
/// Frontmatter key holding the fingerprint of the uncommitted code changes present at sync time.
pub const KEY_SYNCED_DIGEST: &str = "x-anvil-synced-digest";

/// Producer string recorded in `generated.by`, e.g. `anvil/0.4.0`.
pub fn producer() -> String {
    format!("anvil/{}", env!("CARGO_PKG_VERSION"))
}

/// Current time as an RFC 3339 UTC timestamp with millisecond precision.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Current UTC date as `YYYY-MM-DD`.
pub fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}
