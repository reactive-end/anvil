//! `anvil check`: diff-classified Tier 0 / Tier 1 / Tier 2 verification.

mod tier2;

use std::fs;
use std::path::Path;

use anyhow::Result;
use anvil_ast::analyze_files_parallel;
use anvil_core::config::{AnvilConfig, StackKind};
use anvil_core::diff_classifier::{ChangedFile, DiffClassification, classify_files};
use anvil_core::fs_util::bin_in_path;
use anvil_core::git;
use anvil_core::ratchet::{evaluate_promoted_rules, load_promoted_rules};
use anvil_core::traits::{CompressedOutput, Diagnostic, Severity, Tier};
use anvil_graph::CodeGraphEngine;
use anvil_graph::builder::collect_source_files;
use anvil_graph::model::CodeGraph;
use rustc_hash::FxHashSet;

use crate::commands::ExitCodeError;

/// Executes `anvil check [--full] [--tier 0|1|2] [--no-tests]`.
pub fn execute(root: &Path, full: bool, tier_override: Option<u8>, no_tests: bool) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    let (changed_paths, added_paths, classification) = classify_repo_diff(root);
    let effective_tier = resolve_tier(full, tier_override, classification.minimum_tier);

    println!(
        "anvil check: category={:?} tier={:?} changed={} skip_logic_tests={}",
        classification.primary_category,
        effective_tier,
        changed_paths.len(),
        classification.skip_logic_tests
    );

    let tier0_errors = run_tier0(root, &config, &changed_paths, full)?;
    let mut tier1_failed = false;

    if effective_tier >= Tier::Tier1 && !changed_paths.is_empty() {
        tier1_failed = run_tier1(
            root,
            &config,
            &changed_paths,
            &added_paths,
            &classification,
            no_tests,
        )?;
    }
    if effective_tier >= Tier::Tier2 {
        tier2::run_tier2(root, &config, &changed_paths, &classification, full);
    }

    if tier0_errors > 0 || tier1_failed {
        return Err(ExitCodeError::with_message(
            1,
            format!("anvil check failed (tier0_errors={tier0_errors}, tier1_failed={tier1_failed})"),
        )
        .into());
    }
    println!("anvil check: PASS");
    Ok(())
}

fn classify_repo_diff(root: &Path) -> (Vec<String>, Vec<String>, DiffClassification) {
    let status = git::working_tree_status(root).unwrap_or_default();
    let mut unique = FxHashSet::default();
    let mut paths = Vec::new();
    for p in status
        .added
        .iter()
        .chain(&status.modified)
        .chain(&status.deleted)
        .chain(&status.untracked)
    {
        if unique.insert(p.clone()) {
            paths.push(p.clone());
        }
    }
    paths.sort();

    let mut added_set = FxHashSet::default();
    let mut added_paths = Vec::new();
    for p in status.added.iter().chain(&status.untracked) {
        let norm = CodeGraph::normalize_rel_path(p);
        if added_set.insert(norm.clone()) {
            added_paths.push(norm);
        }
    }

    let diffs = git::batched_working_tree_diffs(root).unwrap_or_default();
    let changed: Vec<ChangedFile> = paths
        .iter()
        .map(|p| match diffs.get(p) {
            Some(hunks) if !hunks.is_empty() => ChangedFile::with_hunks(p, hunks.clone()),
            _ => ChangedFile::from_path(p),
        })
        .collect();
    let classification = classify_files(&changed);
    (paths, added_paths, classification)
}

fn resolve_tier(full: bool, tier_override: Option<u8>, minimum: Tier) -> Tier {
    match tier_override {
        Some(0) => Tier::Tier0,
        Some(1) => Tier::Tier1,
        Some(2) => Tier::Tier2,
        _ if full => Tier::Tier2,
        _ => minimum,
    }
}

fn run_tier0(root: &Path, config: &AnvilConfig, changed: &[String], full: bool) -> Result<usize> {
    let targets: Vec<String> = if !changed.is_empty() {
        changed.to_vec()
    } else if full {
        collect_source_files(root)
            .into_iter()
            .map(|(_, rel)| rel)
            .collect()
    } else {
        Vec::new()
    };

    let mut loaded: Vec<(String, String)> = Vec::new();
    for rel in targets {
        let abs = root.join(&rel);
        if abs.is_file()
            && let Ok(source) = fs::read_to_string(&abs)
        {
            loaded.push((rel, source));
        }
    }

    let ast_results = analyze_files_parallel(&loaded, config);
    let promoted_rules = load_promoted_rules(root)?;
    let mut errors = 0usize;

    for ((rel, source), ast_res) in loaded.iter().zip(ast_results) {
        let facts = ast_res?;
        print_diagnostics(&facts.diagnostics, &mut errors);
        if !promoted_rules.is_empty() {
            let rule_diags = evaluate_promoted_rules(&promoted_rules, rel, source);
            print_diagnostics(&rule_diags, &mut errors);
        }
    }
    Ok(errors)
}

fn print_diagnostics(diagnostics: &[Diagnostic], errors: &mut usize) {
    for diag in diagnostics {
        let tag = match diag.severity {
            Severity::Error => {
                *errors += 1;
                "ERROR"
            }
            Severity::Warning => "WARN",
            Severity::Info => "INFO",
        };
        println!(
            "  [{tag}] {}:{}:{} [{}] {}",
            diag.file, diag.line, diag.column, diag.rule, diag.message
        );
    }
}

fn run_tier1(
    root: &Path,
    config: &AnvilConfig,
    changed: &[String],
    added_paths: &[String],
    classification: &DiffClassification,
    no_tests: bool,
) -> Result<bool> {
    let engine = CodeGraphEngine::load_or_build(root, config.clone())?;
    let blast = engine.blast_radius(changed, 2);
    println!(
        "  [Tier 1] blast_radius: {} impacted, {} related test(s) ({:.2}ms)",
        blast.impacted_files.len(),
        blast.related_tests.len(),
        blast.elapsed_ms
    );

    let over = engine.check_overengineering(changed, added_paths, changed.len() <= 3);
    for w in over.warnings {
        println!(
            "  [WARN] overengineering: {} (consumers={}): {}",
            w.file, w.consumer_count, w.reason
        );
    }

    if classification.skip_logic_tests {
        println!("  [Tier 1] style/docs-only diff: skipping logic typecheck/test suites");
        return Ok(false);
    }

    let mut failed = false;
    if has_installed_typechecker(config) {
        let mut scope = blast.changed_files;
        for item in blast.impacted_files {
            if !scope.contains(&item.file) {
                scope.push(item.file);
            }
        }
        failed |= report_compressed("typecheck", &engine.run_impacted_typecheck(&scope)?);
    }

    if !no_tests && !blast.related_tests.is_empty() {
        failed |= report_compressed("test", &engine.run_impacted_tests(&blast.related_tests)?);
    }
    Ok(failed)
}

fn report_compressed(kind: &str, outputs: &[CompressedOutput]) -> bool {
    let mut failed = false;
    for out in outputs {
        println!("  [{kind}:{}] {}", out.tool, out.summary);
        if !out.passed {
            failed = true;
            for line in &out.lines {
                println!("    {line}");
            }
        }
    }
    failed
}

fn has_installed_typechecker(config: &AnvilConfig) -> bool {
    let needs_tsc = config.stacks.iter().any(|s| {
        matches!(
            s,
            StackKind::TypeScript
                | StackKind::React
                | StackKind::Vite
                | StackKind::NextJs
                | StackKind::Astro
                | StackKind::Angular
                | StackKind::NestJs
                | StackKind::ExpoReactNative
        )
    });
    if needs_tsc && bin_in_path("tsc").is_some() {
        return true;
    }
    if config.stacks.contains(&StackKind::Python) && bin_in_path("pyright").is_some() {
        return true;
    }
    config.stacks.contains(&StackKind::FlutterDart) && bin_in_path("dart").is_some()
}
