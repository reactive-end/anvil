//! `anvil impact`: reverse-BFS blast radius, upstream origin tracing, and filtered typecheck.

use std::path::Path;

use anyhow::Result;
use anvil_core::config::AnvilConfig;
use anvil_core::git::{self, DiffScope};
use anvil_graph::CodeGraphEngine;
use anvil_graph::impact::BlastRadiusReport;
use anvil_graph::model::CodeGraph;

use crate::commands::ExitCodeError;

/// Executes `anvil impact [files...] [--depth N] [--origin <sym>] [--typecheck]`.
pub fn execute(
    root: &Path,
    files: &[String],
    depth: usize,
    origin: Option<&str>,
    typecheck: bool,
) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    let engine = CodeGraphEngine::load_cached(root, config)?;

    if let Some(sym) = origin {
        print_origin(&engine, sym);
        if files.is_empty() && !typecheck {
            return Ok(());
        }
    }

    let target_files: Vec<String> = if files.is_empty() {
        git::changed_files(root, DiffScope::WorkingTree).unwrap_or_default()
    } else {
        files
            .iter()
            .map(|f| CodeGraph::normalize_rel_path(f))
            .collect()
    };

    if target_files.is_empty() {
        println!("no modified or target files to analyze for blast radius");
        return Ok(());
    }

    let report = engine.blast_radius(&target_files, depth);
    print_blast_report(&report);

    if typecheck && !run_impact_typecheck(&engine, &report)? {
        return Err(ExitCodeError::with_message(1, "anvil impact --typecheck failed").into());
    }
    Ok(())
}

fn print_blast_report(report: &BlastRadiusReport) {
    println!(
        "blast radius (depth={}, {:.2}ms): {} changed -> {} impacted file(s), {} related test(s)",
        report.max_depth,
        report.elapsed_ms,
        report.changed_files.len(),
        report.impacted_files.len(),
        report.related_tests.len()
    );
    for item in &report.impacted_files {
        println!(
            "  [d={}{}] {} ({})",
            item.depth,
            if item.is_screen { ",screen" } else { "" },
            item.file,
            item.via_edges.join("; ")
        );
    }
    for test in &report.related_tests {
        println!("  [test] {test}");
    }
}

fn run_impact_typecheck(engine: &CodeGraphEngine, report: &BlastRadiusReport) -> Result<bool> {
    let mut scope = report.changed_files.clone();
    for item in &report.impacted_files {
        if !scope.contains(&item.file) {
            scope.push(item.file.clone());
        }
    }
    let mut all_passed = true;
    for out in engine.run_impacted_typecheck(&scope)? {
        println!("typecheck [{}]: {}", out.tool, out.summary);
        for line in &out.lines {
            println!("  {line}");
        }
        if !out.passed {
            all_passed = false;
        }
    }
    Ok(all_passed)
}

fn print_origin(engine: &CodeGraphEngine, sym: &str) {
    match engine.find_origin(sym) {
        Some(res) => {
            println!(
                "origin for `{}` -> {} ({}:L{}) hops={}",
                res.queried,
                res.origin_label,
                res.origin_file,
                res.origin_line,
                res.hops.join(" -> ")
            );
        }
        None => println!("origin for `{sym}`: not found in graph"),
    }
}
