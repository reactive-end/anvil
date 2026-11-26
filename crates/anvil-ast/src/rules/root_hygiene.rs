//! Tier 0 rule: enforce `AnvilConfig::root_allowlist` for root-level repository files.

use std::path::{Component, Path};

use anvil_core::config::AnvilConfig;
use anvil_core::traits::{Diagnostic, Severity};

/// Checks a repo-relative path to ensure root-level files belong to `config.root_allowlist`.
///
/// Absolute paths are ignored because repository root depth cannot be inferred without `repo_root`.
pub fn check_root_hygiene(
    path: &Path,
    normalized_file: &str,
    config: &AnvilConfig,
) -> Option<Diagnostic> {
    if path.is_absolute() {
        return None;
    }

    let mut normal_parts = path.components().filter_map(|comp| match comp {
        Component::Normal(s) => s.to_str(),
        _ => None,
    });

    let first = normal_parts.next()?;
    if normal_parts.next().is_some() {
        return None;
    }

    let allowed = config.root_allowlist.iter().any(|entry| matches_allowlist(entry, first));

    if allowed {
        return None;
    }

    Some(Diagnostic {
        rule: "root-hygiene".to_owned(),
        severity: Severity::Error,
        file: normalized_file.to_owned(),
        line: 1,
        column: 1,
        message: format!(
            "Root-level file `{first}` is not in `.harness/config.json` root_allowlist"
        ),
        suggestion: Some(
            "Move the file into an appropriate subdirectory or add it to `root_allowlist`"
                .to_owned(),
        ),
    })
}

fn matches_allowlist(pattern: &str, file_name: &str) -> bool {
    if pattern == file_name {
        return true;
    }
    if let Some((prefix, suffix)) = pattern.split_once('*')
        && !suffix.contains('*')
        && file_name.len() >= prefix.len() + suffix.len()
    {
        return file_name.starts_with(prefix) && file_name.ends_with(suffix);
    }
    false
}
