//! Tier 0 documentation checks: OKF validity of changed docs, and stale concepts the change touches.

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_core::traits::{Diagnostic, Severity};
use anvil_docs::bundle;
use anvil_docs::concept::scan_all;
use anvil_docs::drift::Drift;
use anvil_docs::paths::PathMatcher;
use anvil_docs::repo_files;
use anvil_docs::validate::{self, Severity as DocSeverity};

const DOCS_PREFIX: &str = ".harness/docs/";

/// Validation errors of changed docs (errors) and stale concepts documenting changed code (warnings).
pub fn diagnostics(root: &Path, config: &AnvilConfig, changed: &[String]) -> Vec<Diagnostic> {
    if !bundle::has_bundles(root) || changed.is_empty() {
        return Vec::new();
    }
    let mut out = invalid_docs(root, config, changed);
    out.extend(stale_concepts(root, config, changed));
    out
}

fn diag(file: &str, severity: Severity, rule: &str, message: String) -> Diagnostic {
    Diagnostic {
        rule: rule.to_owned(),
        severity,
        file: file.to_owned(),
        line: 1,
        column: 1,
        message,
        suggestion: None,
    }
}

fn invalid_docs(root: &Path, config: &AnvilConfig, changed: &[String]) -> Vec<Diagnostic> {
    let docs: Vec<&String> = changed
        .iter()
        .filter(|p| p.starts_with(DOCS_PREFIX) && p.ends_with(".md") && root.join(p).is_file())
        .collect();
    if docs.is_empty() {
        return Vec::new();
    }
    let bundles = bundle::existing(root, config);
    let files = repo_files::list(root);
    let mut out = Vec::new();
    for rel in docs {
        let Some(found) = bundle::project_of_doc_path(rel)
            .and_then(|project| bundles.iter().find(|b| b.project == project))
        else {
            continue;
        };
        for issue in validate::validate_file(root, found, &root.join(rel), &files) {
            let severity = match issue.severity {
                DocSeverity::Error => Severity::Error,
                DocSeverity::Warning => Severity::Warning,
            };
            out.push(diag(rel, severity, "docs/okf", issue.message));
        }
    }
    out
}

fn stale_concepts(root: &Path, config: &AnvilConfig, changed: &[String]) -> Vec<Diagnostic> {
    let code: Vec<&String> = changed.iter().filter(|p| !p.starts_with(DOCS_PREFIX)).collect();
    if code.is_empty() {
        return Vec::new();
    }
    let bundles = bundle::existing(root, config);
    let mut drift = Drift::new(root);
    let mut out = Vec::new();
    for concept in scan_all(root, &bundles, false) {
        let Ok(matcher) = PathMatcher::new(&concept.doc.code_paths()) else {
            continue;
        };
        if matcher.is_empty() || !code.iter().any(|p| matcher.is_match(p)) {
            continue;
        }
        if let Some(reason) = drift.check(&concept).reason() {
            let message = format!(
                "stale: {reason}; update it if behaviour changed, then run `anvil docs refresh {}`",
                concept.key()
            );
            out.push(diag(&concept.rel_path, Severity::Warning, "docs/stale", message));
        }
    }
    out
}
