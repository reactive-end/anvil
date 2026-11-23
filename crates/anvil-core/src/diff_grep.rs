//! Compressed, noise-filtered diff and grep utilities for token-bounded context.

pub mod diff;
pub mod grep;

pub use diff::{CompressedDiff, compress_diff};
pub use grep::{CompressedGrep, compressed_grep};

const NOISE_EXACT_NAMES: [&str; 10] = [
    "Cargo.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "poetry.lock",
    "pubspec.lock",
    "composer.lock",
    ".phpunit.result.cache",
    "graph.json",
    "graph.bin",
];

const NOISE_DIR_SEGMENTS: [&str; 7] = [
    "dist",
    "build",
    "node_modules",
    "target",
    ".next",
    "vendor",
    "storage",
];

/// Returns true for lockfiles, minified/sourcemap files, generated/vendor dirs, and graph artifacts.
pub fn is_noise_file(rel_path: &str) -> bool {
    let norm = normalize_slashed(rel_path);
    if norm.is_empty() {
        return false;
    }

    if norm == ".harness/.state"
        || norm.starts_with(".harness/.state/")
        || norm.contains("/.harness/.state/")
    {
        return true;
    }

    let trailing_slash = norm.ends_with('/');
    let segments: Vec<&str> = norm.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return false;
    }

    let dir_count = if trailing_slash {
        segments.len()
    } else {
        segments.len().saturating_sub(1)
    };
    if segments[..dir_count]
        .iter()
        .any(|seg| NOISE_DIR_SEGMENTS.contains(seg))
    {
        return true;
    }

    if trailing_slash {
        return false;
    }

    let file_name = segments[segments.len() - 1];
    if NOISE_EXACT_NAMES.contains(&file_name) {
        return true;
    }

    let lower = file_name.to_ascii_lowercase();
    lower.ends_with(".min.js") || lower.ends_with(".min.css") || lower.ends_with(".map")
}

pub(crate) fn normalize_slashed(raw: &str) -> String {
    let replaced = raw.trim().replace('\\', "/");
    replaced
        .strip_prefix("./")
        .unwrap_or(&replaced)
        .trim_start_matches('/')
        .to_owned()
}
