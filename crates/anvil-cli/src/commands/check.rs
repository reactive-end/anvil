//! `anvil check`: diff-classified Tier 0 / Tier 1 / Tier 2 verification.

mod tier1;
mod tier2;

use std::fs;
use std::path::Path;

use anyhow::Result;
use anvil_ast::analyze_files_parallel;
use anvil_core::config::AnvilConfig;
use anvil_core::diff_classifier::{ChangedFile, DiffClassification, classify_files};
use anvil_core::git::{self, DiffScope};
use anvil_core::ratchet::{compile_promoted_rules, evaluate_compiled, load_promoted_rules};
use anvil_core::traits::{Diagnostic, Severity, Tier};
use anvil_graph::builder::collect_source_files;
use anvil_graph::model::CodeGraph;
use rustc_hash::FxHashSet;

use crate::commands::ExitCodeError;
use crate::ui;
use tier1::{Tier1Options, run_tier1};

/// Executes `anvil check [--full] [--tier 0|1|2] [--no-tests] [--no-cache] [--staged]`.
pub fn execute(
    root: &Path,
    full: bool,
    tier_override: Option<u8>,
    no_tests: bool,
    no_cache: bool,
    staged: bool,
) -> Result<()> {
    ui::print_banner_if_tty("Tiered Verification Pipeline");
    let config = AnvilConfig::load_or_default(root)?;
    let (changed_paths, added_paths, classification) = if staged {
        classify_staged_diff(root)
    } else {
        classify_repo_diff(root)
    };
    let effective_tier = resolve_tier(full, tier_override, classification.minimum_tier);

    println!(
        "{} category={} tier={} changed={} skip_logic_tests={}",
        ui::accent("anvil check:"),
        ui::cyan(&format!("{:?}", classification.primary_category)),
        ui::bold(&format!("{effective_tier:?}")),
        ui::cyan(&changed_paths.len().to_string()),
        classification.skip_logic_tests
    );

    let tier0_errors = run_tier0(root, &config, &changed_paths, full)?;
    let mut tier1_failed = false;

    if effective_tier >= Tier::Tier1 && (full || !changed_paths.is_empty()) {
        ui::print_section("Tier 1 · Impact, Typecheck & Tests");
        tier1_failed = run_tier1(
            root,
            &config,
            &changed_paths,
            &added_paths,
            &classification,
            Tier1Options {
                full,
                no_tests,
                no_cache,
                tier0_passed: tier0_errors == 0,
            },
        )?;
    }
    let mut tier2_failed = false;
    if effective_tier >= Tier::Tier2 {
        ui::print_section("Tier 2 · Dependencies & Visual QA");
        tier2_failed = tier2::run_tier2(root, &config, &changed_paths, &classification, full);
    }

    if tier0_errors > 0 || tier1_failed || tier2_failed {
        return Err(ExitCodeError::with_message(
            1,
            format!(
                "anvil check failed (tier0_errors={tier0_errors}, tier1_failed={tier1_failed}, tier2_failed={tier2_failed})"
            ),
        )
        .into());
    }
    if ui::colors_enabled() {
        println!();
    }
    println!("{}", ui::green("✓ anvil check: PASS"));
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

fn classify_staged_diff(root: &Path) -> (Vec<String>, Vec<String>, DiffClassification) {
    let mut paths = git::changed_files(root, DiffScope::Staged).unwrap_or_default();
    paths.sort();
    paths.dedup();
    let changed: Vec<ChangedFile> = paths.iter().map(ChangedFile::from_path).collect();
    let classification = classify_files(&changed);
    (paths, Vec::new(), classification)
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
    let targets: Vec<String> = if full {
        collect_source_files(root)
            .into_iter()
            .map(|(_, rel)| rel)
            .collect()
    } else if !changed.is_empty() {
        changed.to_vec()
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
    let compiled_rules = compile_promoted_rules(&promoted_rules);
    let mut errors = 0usize;

    for ((rel, source), ast_res) in loaded.iter().zip(ast_results) {
        let facts = ast_res?;
        print_diagnostics(&facts.diagnostics, &mut errors);
        if !compiled_rules.is_empty() {
            let rule_diags = evaluate_compiled(&compiled_rules, rel, source);
            print_diagnostics(&rule_diags, &mut errors);
        }
    }
    Ok(errors)
}

fn print_diagnostics(diagnostics: &[Diagnostic], errors: &mut usize) {
    for diag in diagnostics {
        let badge = match diag.severity {
            Severity::Error => {
                *errors += 1;
                ui::error_badge()
            }
            Severity::Warning => ui::warn_badge(),
            Severity::Info => ui::info_badge(),
        };
        println!(
            "  {badge} {}:{}:{} {} {}",
            ui::cyan(&diag.file),
            diag.line,
            diag.column,
            ui::dim(&format!("[{}]", diag.rule)),
            diag.message
        );
    }
}
