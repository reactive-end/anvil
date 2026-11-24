//! `anvil check`: diff-classified Tier 0 / Tier 1 / Tier 2 verification.

mod tier1;
mod tier2;

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::Result;
use anvil_ast::analyze_files_parallel;
use anvil_core::config::AnvilConfig;
use anvil_core::diff_classifier::{ChangedFile, DiffClassification, classify_files};
use anvil_core::git::{self, DiffScope};
use anvil_core::ratchet::{compile_promoted_rules, evaluate_compiled, load_promoted_rules};
use anvil_core::sonar::{check_cognitive_complexity, check_new_code_duplication};
use anvil_core::test_tampering::{check_test_tampering_diff, is_test_file};
use anvil_core::traits::{Diagnostic, Severity, Tier};
use anvil_graph::builder::collect_source_files;
use anvil_graph::model::CodeGraph;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::commands::ExitCodeError;
use crate::ui;
use tier1::{Tier1Options, run_tier1};

/// Options for `anvil check`.
#[derive(Debug, Clone, Copy, Default)]
pub struct CheckOptions {
    pub full: bool,
    pub tier_override: Option<u8>,
    pub no_tests: bool,
    pub no_cache: bool,
    pub staged: bool,
    pub sonar: bool,
    pub no_sonar: bool,
}

/// Executes `anvil check [--full] [--tier 0|1|2] [--no-tests] [--no-cache] [--staged] [--sonar] [--no-sonar]`.
pub fn execute(root: &Path, opts: CheckOptions) -> Result<()> {
    ui::print_banner_if_tty("Tiered Verification Pipeline");
    let config = AnvilConfig::load_or_default(root)?;
    let run_sonar = (config.sonar.enabled || opts.sonar) && !opts.no_sonar;
    let (changed_paths, added_paths, classification) = if opts.staged {
        classify_staged_diff(root)
    } else {
        classify_repo_diff(root)
    };
    let effective_tier = resolve_tier(opts.full, opts.tier_override, classification.minimum_tier);

    println!(
        "{} category={} tier={} changed={} skip_logic_tests={}",
        ui::accent("anvil check:"),
        ui::cyan(&format!("{:?}", classification.primary_category)),
        ui::bold(&format!("{effective_tier:?}")),
        ui::cyan(&changed_paths.len().to_string()),
        classification.skip_logic_tests
    );

    let tier0_errors = run_tier0(
        root,
        &config,
        &changed_paths,
        opts.full,
        opts.staged,
        run_sonar,
    )?;
    let mut tier1_failed = false;

    if effective_tier >= Tier::Tier1 && (opts.full || !changed_paths.is_empty()) {
        ui::print_section("Tier 1 · Impact, Typecheck & Tests");
        tier1_failed = run_tier1(
            root,
            &config,
            &changed_paths,
            &added_paths,
            &classification,
            Tier1Options {
                full: opts.full,
                no_tests: opts.no_tests,
                no_cache: opts.no_cache,
                tier0_passed: tier0_errors == 0,
            },
        )?;
    }
    let mut tier2_failed = false;
    if effective_tier >= Tier::Tier2 || (opts.sonar && !opts.no_sonar) {
        ui::print_section("Tier 2 · Dependencies & Visual QA");
        tier2_failed =
            tier2::run_tier2(root, &config, &changed_paths, &classification, opts.full, run_sonar);
    }

    finish_check(tier0_errors, tier1_failed, tier2_failed)
}

fn finish_check(tier0_errors: usize, tier1_failed: bool, tier2_failed: bool) -> Result<()> {
    if tier0_errors > 0 || tier1_failed || tier2_failed {
        return Err(ExitCodeError::with_message(
            1,
            format!("anvil check failed (tier0_errors={tier0_errors}, tier1_failed={tier1_failed}, tier2_failed={tier2_failed})"),
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

fn run_tier0(
    root: &Path,
    config: &AnvilConfig,
    changed: &[String],
    full: bool,
    staged: bool,
    run_sonar: bool,
) -> Result<usize> {
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
    for rel in &targets {
        let abs = root.join(rel);
        if abs.is_file()
            && let Ok(source) = fs::read_to_string(&abs)
        {
            loaded.push((rel.clone(), source));
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
    run_tampering_and_sonar(root, config, changed, staged, run_sonar, &loaded, &mut errors);
    Ok(errors)
}

fn run_tampering_and_sonar(
    root: &Path,
    config: &AnvilConfig,
    changed: &[String],
    staged: bool,
    run_sonar: bool,
    loaded: &[(String, String)],
    errors: &mut usize,
) {
    let test_diffs = collect_test_diffs(root, changed, staged);
    for (rel, diff_text) in test_diffs {
        let diags = check_test_tampering_diff(&rel, &diff_text);
        print_diagnostics(&diags, errors);
    }
    if run_sonar {
        for (rel, src) in loaded {
            let diags =
                check_cognitive_complexity(rel, src, config.sonar.cognitive_complexity_max);
            print_diagnostics(&diags, errors);
        }
        let dup_diags = check_new_code_duplication(loaded, config.sonar.duplication_min_lines);
        print_diagnostics(&dup_diags, errors);
    }
}

fn collect_test_diffs(
    root: &Path,
    changed: &[String],
    staged: bool,
) -> FxHashMap<String, String> {
    let test_paths: Vec<&String> = changed.iter().filter(|p| is_test_file(p)).collect();
    let mut out = FxHashMap::default();
    if test_paths.is_empty() {
        return out;
    }
    if staged {
        for rel in test_paths {
            if let Ok(output) = Command::new("git")
                .args(["diff", "--cached", "--unified=1", "--", rel])
                .current_dir(root)
                .output()
                && output.status.success()
            {
                out.insert(
                    rel.clone(),
                    String::from_utf8_lossy(&output.stdout).into_owned(),
                );
            }
        }
        return out;
    }
    let diffs = git::batched_working_tree_diffs(root).unwrap_or_default();
    for rel in test_paths {
        if let Some(diff_text) = diffs.get(rel) {
            out.insert(rel.clone(), diff_text.clone());
        }
    }
    out
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
