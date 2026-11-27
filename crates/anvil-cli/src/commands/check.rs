//! `anvil check`: diff-classified Tier 0 / Tier 1 / Tier 2 verification.

mod api_tier;
mod docs;
mod tier1;
mod tier2;

use std::fs;
use std::path::Path;

use anvil_ast::analyze_files_parallel;
use anvil_ast::rules::retain_new_code_findings;
use anvil_core::config::AnvilConfig;
use anvil_core::diff_classifier::{ChangedFile, DiffClassification, classify_files};
use anvil_core::duplication::check_added_duplication;
use anvil_core::git::{self, AddedLines, DiffScope};
use anvil_core::orchestrator_mode::scope_allows;
use anvil_core::ratchet::{compile_promoted_rules, evaluate_compiled, load_promoted_rules};
use anvil_core::sonar::check_cognitive_complexity;
use anvil_core::stats::record_check_outcome;
use anvil_core::test_tampering::{check_test_tampering_diff, is_test_file};
use anvil_core::traits::{Diagnostic, Severity, Tier};
use anvil_graph::builder::collect_source_files;
use anvil_graph::model::CodeGraph;
use anyhow::Result;
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
///
/// A non-empty `scope` limits the check to changes inside those files or directories.
pub fn execute(root: &Path, opts: CheckOptions, scope: &[String]) -> Result<()> {
    ui::print_banner_if_tty("Tiered Verification Pipeline");
    let config = AnvilConfig::load_or_default(root)?;
    let any_sonar = config.sonar.enabled || config.subrepos.iter().any(|s| s.sonar_enabled);
    let run_sonar = (any_sonar || opts.sonar) && !opts.no_sonar;
    let keep = |path: &str| scope.is_empty() || scope_allows(scope, path);
    let (changed_paths, added_paths, classification) = if opts.staged {
        classify_staged_diff(root, &keep)
    } else {
        classify_repo_diff(root, &keep)
    };
    let mut added_lines = git::added_line_ranges(root, opts.staged).unwrap_or_default();
    added_lines.retain(|path, _| keep(path));
    let base_tier = resolve_tier(opts.full, opts.tier_override, classification.minimum_tier);
    let api_raise =
        api_tier::raises_to_tier2(&config, opts.tier_override, base_tier, &changed_paths);
    let effective_tier = if api_raise { Tier::Tier2 } else { base_tier };

    println!(
        "{} category={} tier={} changed={} skip_logic_tests={}",
        ui::accent("anvil check:"),
        ui::cyan(&format!("{:?}", classification.primary_category)),
        ui::bold(&format!("{effective_tier:?}")),
        ui::cyan(&changed_paths.len().to_string()),
        classification.skip_logic_tests
    );
    if api_raise {
        println!("  tier raised to Tier2: qa.api.paths touched (--tier 0|1 opts out)");
    }

    let tier0_errors =
        run_timed_tier0(root, &config, &changed_paths, opts, run_sonar, &added_lines)?;
    let mut tier1_failed = false;

    if effective_tier >= Tier::Tier1 && (opts.full || !changed_paths.is_empty()) {
        ui::print_section("Tier 1 · Impact, Typecheck & Tests");
        tier1_failed = run_tier1(
            root,
            &config,
            &changed_paths,
            &added_paths,
            &added_lines,
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
        // Raising the tier for the API must not start Sonar, which only ran at Tier 2 or on request.
        let sonar_now = run_sonar && (base_tier >= Tier::Tier2 || (opts.sonar && !opts.no_sonar));
        tier2_failed =
            tier2::run_tier2(root, &config, &changed_paths, &classification, opts.full, sonar_now);
    }

    let passed = tier0_errors == 0 && !tier1_failed && !tier2_failed;
    let _ = record_check_outcome(root, passed, &format!("{effective_tier:?}"), &changed_paths);
    finish_check(tier0_errors, tier1_failed, tier2_failed)
}

fn run_timed_tier0(
    root: &Path,
    config: &AnvilConfig,
    changed: &[String],
    opts: CheckOptions,
    run_sonar: bool,
    added_lines: &AddedLines,
) -> Result<usize> {
    let started = std::time::Instant::now();
    let errors = run_tier0(root, config, changed, opts.full, opts.staged, run_sonar, added_lines)?;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    if elapsed_ms > config.tier_budgets.tier0_ms {
        println!(
            "  {} Tier 0 took {elapsed_ms}ms (budget {}ms)",
            ui::warn_badge(),
            config.tier_budgets.tier0_ms
        );
    }
    Ok(errors)
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

fn classify_repo_diff(
    root: &Path,
    keep: &dyn Fn(&str) -> bool,
) -> (Vec<String>, Vec<String>, DiffClassification) {
    let status = git::working_tree_status(root).unwrap_or_default();
    let mut unique = FxHashSet::default();
    let mut paths = Vec::new();
    for p in
        status.added.iter().chain(&status.modified).chain(&status.deleted).chain(&status.untracked)
    {
        if keep(p) && unique.insert(p.clone()) {
            paths.push(p.clone());
        }
    }
    paths.sort();

    let mut added_set = FxHashSet::default();
    let mut added_paths = Vec::new();
    for p in status.added.iter().chain(&status.untracked).filter(|p| keep(p)) {
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

fn classify_staged_diff(
    root: &Path,
    keep: &dyn Fn(&str) -> bool,
) -> (Vec<String>, Vec<String>, DiffClassification) {
    let mut paths = git::changed_files(root, DiffScope::Staged).unwrap_or_default();
    paths.retain(|p| keep(p));
    paths.sort();
    paths.dedup();
    let diffs = git::batched_staged_diffs(root).unwrap_or_default();
    let changed: Vec<ChangedFile> = paths
        .iter()
        .map(|p| match diffs.get(p) {
            Some(hunks) if !hunks.is_empty() => ChangedFile::with_hunks(p, hunks.clone()),
            _ => ChangedFile::from_path(p),
        })
        .collect();
    let classification = classify_files(&changed);
    let added = git::working_tree_status(root).unwrap_or_default().added;
    let added_paths = added
        .iter()
        .filter(|p| paths.contains(p))
        .map(|p| CodeGraph::normalize_rel_path(p))
        .collect();
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

fn run_tier0(
    root: &Path,
    config: &AnvilConfig,
    changed: &[String],
    full: bool,
    staged: bool,
    run_sonar: bool,
    added_lines: &AddedLines,
) -> Result<usize> {
    let targets: Vec<String> = if full {
        collect_source_files(root).into_iter().map(|(_, rel)| rel).collect()
    } else if !changed.is_empty() {
        changed.to_vec()
    } else {
        Vec::new()
    };

    let mut loaded: Vec<(String, String)> = Vec::new();
    for rel in &targets {
        let abs = root.join(rel);
        if let Some(source) = read_bounded_source(&abs) {
            loaded.push((rel.clone(), source));
        }
    }

    let ast_results = analyze_files_parallel(&loaded, config);
    let promoted_rules = load_promoted_rules(root)?;
    let compiled_rules = compile_promoted_rules(&promoted_rules);
    let mut errors = 0usize;

    for ((rel, source), ast_res) in loaded.iter().zip(ast_results) {
        let mut diagnostics = ast_res?.diagnostics;
        retain_new_code_findings(rel, &mut diagnostics, Some(added_lines));
        print_diagnostics(&diagnostics, &mut errors);
        if !compiled_rules.is_empty() {
            let rule_diags = evaluate_compiled(&compiled_rules, rel, source);
            print_diagnostics(&rule_diags, &mut errors);
        }
    }
    run_tampering_and_sonar(root, config, changed, staged, run_sonar, &loaded, &mut errors);
    print_diagnostics(&docs::diagnostics(root, config, changed), &mut errors);
    if !added_lines.is_empty() {
        let repo_files: Vec<String> =
            collect_source_files(root).into_iter().map(|(_, rel)| rel).collect();
        let duplicates =
            check_added_duplication(root, &repo_files, added_lines, config.duplication_window());
        print_diagnostics(&duplicates, &mut errors);
    }
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
            let diags = check_cognitive_complexity(rel, src, config.sonar.cognitive_complexity_max);
            print_diagnostics(&diags, errors);
        }
    }
}

fn collect_test_diffs(root: &Path, changed: &[String], staged: bool) -> FxHashMap<String, String> {
    let test_paths: Vec<&String> = changed.iter().filter(|p| is_test_file(p)).collect();
    let mut out = FxHashMap::default();
    if test_paths.is_empty() {
        return out;
    }
    let diffs = if staged {
        git::batched_staged_diffs(root).unwrap_or_default()
    } else {
        git::pending_diffs(root).unwrap_or_default()
    };
    for rel in test_paths {
        if let Some(diff_text) = diffs.get(rel) {
            out.insert(rel.clone(), diff_text.clone());
        }
    }
    out
}

const MAX_SOURCE_SCAN_BYTES: u64 = 2 * 1024 * 1024;

pub(crate) fn read_bounded_source(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_SOURCE_SCAN_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
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
