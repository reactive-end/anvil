//! `anvil docs` commands that read: list, check, search.

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_docs::bundle;
use anvil_docs::concept::scan_all;
use anvil_docs::drift::Drift;
use anvil_docs::repo_files;
use anvil_docs::search as docs_search;
use anvil_docs::trust::trust_of;
use anvil_docs::validate::{Issue, Severity, validate_bundle};
use anyhow::Result;
use serde::Serialize;

use crate::commands::ExitCodeError;

#[derive(Debug, Serialize)]
struct ListRow {
    project: String,
    id: String,
    path: String,
    status: String,
    trust: String,
    stale: bool,
    stale_reason: Option<String>,
}

fn rows(root: &Path, config: &AnvilConfig) -> Vec<ListRow> {
    let bundles = bundle::existing(root, config);
    let mut drift = Drift::new(root);
    scan_all(root, &bundles, false)
        .iter()
        .map(|c| {
            let freshness = drift.check(c);
            ListRow {
                project: c.project.clone(),
                id: c.key(),
                path: c.rel_path.clone(),
                status: c.doc.status().unwrap_or("-").to_owned(),
                trust: trust_of(&c.doc).to_string(),
                stale: freshness.is_stale(),
                stale_reason: freshness.reason().map(str::to_owned),
            }
        })
        .collect()
}

/// `anvil docs list [--json]`.
pub fn list(root: &Path, config: &AnvilConfig, json: bool) -> Result<()> {
    let rows = rows(root, config);
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("docs: no concepts (run `anvil docs init` and `anvil docs scaffold`)");
    }
    for row in &rows {
        let state = match &row.stale_reason {
            Some(reason) => format!("STALE ({reason})"),
            None => "fresh".to_owned(),
        };
        println!("{}  [{}]  {}  {state}", row.id, row.status, row.trust);
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct CheckReport {
    errors: Vec<Issue>,
    warnings: Vec<Issue>,
    stale: Vec<ListRow>,
}

/// `anvil docs check [--strict] [--json]`: validation errors fail; stale concepts fail only with `--strict`.
pub fn check(root: &Path, config: &AnvilConfig, strict: bool, json: bool) -> Result<()> {
    let bundles = bundle::existing(root, config);
    let files = repo_files::list(root);
    let issues: Vec<Issue> =
        bundles.iter().flat_map(|b| validate_bundle(root, b, &files)).collect();
    let (errors, warnings): (Vec<Issue>, Vec<Issue>) =
        issues.into_iter().partition(|i| i.severity == Severity::Error);
    let stale: Vec<ListRow> = rows(root, config).into_iter().filter(|r| r.stale).collect();
    let failed = !errors.is_empty() || (strict && !stale.is_empty());
    let report = CheckReport { errors, warnings, stale };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_check(&report, bundles.len());
    }
    if failed {
        return Err(ExitCodeError::with_message(
            1,
            format!(
                "docs check failed ({} error(s), {} stale concept(s))",
                report.errors.len(),
                report.stale.len()
            ),
        )
        .into());
    }
    Ok(())
}

fn print_check(report: &CheckReport, bundle_count: usize) {
    for issue in &report.errors {
        println!("error: {}: {}", issue.file, issue.message);
    }
    for issue in &report.warnings {
        println!("warning: {}: {}", issue.file, issue.message);
    }
    for row in &report.stale {
        println!(
            "warning: {}: stale: {}",
            row.path,
            row.stale_reason.as_deref().unwrap_or("unknown")
        );
    }
    if report.errors.is_empty() && report.warnings.is_empty() && report.stale.is_empty() {
        println!("docs: {bundle_count} bundle(s) valid and fresh");
    }
}

/// `anvil docs search <query> [--limit N] [--json]`.
pub fn search(
    root: &Path,
    config: &AnvilConfig,
    query: &str,
    limit: usize,
    json: bool,
) -> Result<()> {
    let hits = docs_search::search_repo(root, config, query, limit);
    if json {
        println!("{}", serde_json::to_string_pretty(&hits)?);
        return Ok(());
    }
    if hits.is_empty() {
        println!("docs: no match for `{query}`");
    }
    for hit in &hits {
        println!("{:.3}  {}  ({})", hit.score, hit.id, hit.path);
        if !hit.snippet.is_empty() {
            println!("    {}", hit.snippet);
        }
    }
    Ok(())
}
