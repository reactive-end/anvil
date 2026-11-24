//! Tier 1 blast-radius, content-addressed check cache, typecheck, and targeted test runner.

use std::path::Path;

use anyhow::Result;
use anvil_core::check_cache::{self, CachedTier1};
use anvil_core::config::{AnvilConfig, StackKind};
use anvil_core::diff_classifier::DiffClassification;
use anvil_core::fs_util::{
    resolve_godot_bin, resolve_local_or_path_bin, resolve_php_bin, resolve_php_vendor_script,
};
use anvil_core::stats::{StatsEvent, record_event};
use anvil_core::traits::CompressedOutput;
use anvil_graph::CodeGraphEngine;
use anvil_graph::impact::BlastRadiusReport;
use chrono::Utc;

use crate::ui;

#[derive(Clone, Copy)]
pub(super) struct Tier1Options {
    pub full: bool,
    pub no_tests: bool,
    pub no_cache: bool,
    pub tier0_passed: bool,
}

pub(super) fn run_tier1(
    root: &Path,
    config: &AnvilConfig,
    changed: &[String],
    added_paths: &[String],
    classification: &DiffClassification,
    opts: Tier1Options,
) -> Result<bool> {
    let engine = CodeGraphEngine::load_or_build(root, config.clone())?;
    let blast = engine.blast_radius(changed, 2);
    print_blast_summary(&blast);
    if !changed.is_empty() {
        print_overengineering_warnings(&engine, changed, added_paths);
    }
    if !opts.full && classification.skip_logic_tests {
        println!(
            "  {} {}",
            ui::cyan("[Tier 1]"),
            ui::dim("style/docs-only diff: skipping logic typecheck/test suites")
        );
        return Ok(false);
    }

    let (tc_scope, cache_scope) = build_tier1_scopes(&blast);
    let boundary_diags = engine.check_boundaries(changed);
    let boundaries_failed = print_boundary_diagnostics(&boundary_diags);
    let salt = format!(
        "full={};no_tests={};stacks={:?};boundaries={};cycles={}",
        opts.full,
        opts.no_tests,
        config.stacks,
        config.boundaries.len(),
        config.forbid_import_cycles
    );
    let subrepo_paths: Vec<String> = config.subrepos.iter().map(|s| s.path.clone()).collect();
    let cache_key =
        check_cache::compute_scope_key_with_subrepos(root, &cache_scope, &subrepo_paths, &salt).ok();
    if !boundaries_failed && !opts.no_cache && try_tier1_cache_hit(root, cache_key.as_deref()) {
        return Ok(false);
    }

    let mut failed = boundaries_failed;
    if has_installed_typechecker(root, config) {
        failed |= report_compressed("typecheck", &engine.run_impacted_typecheck(&tc_scope)?);
    }
    if !opts.no_tests && !blast.related_tests.is_empty() {
        failed |= report_compressed("test", &engine.run_impacted_tests(&blast.related_tests)?);
    }
    if !failed && opts.tier0_passed {
        store_tier1_cache(root, cache_key.as_deref());
    }
    Ok(failed)
}

fn print_boundary_diagnostics(diags: &[anvil_core::traits::Diagnostic]) -> bool {
    for d in diags {
        println!(
            "  {} [{}] {}:{} — {}",
            ui::error_badge(),
            d.rule,
            d.file,
            d.line,
            d.message
        );
    }
    !diags.is_empty()
}

fn print_blast_summary(blast: &BlastRadiusReport) {
    println!(
        "  {} blast_radius: {} impacted, {} related test(s) {}",
        ui::cyan("[Tier 1]"),
        ui::bold(&blast.impacted_files.len().to_string()),
        ui::bold(&blast.related_tests.len().to_string()),
        ui::dim(&format!("({:.2}ms)", blast.elapsed_ms))
    );
}

fn print_overengineering_warnings(
    engine: &CodeGraphEngine,
    changed: &[String],
    added_paths: &[String],
) {
    let over = engine.check_overengineering(changed, added_paths, changed.len() <= 3);
    for w in over.warnings {
        println!(
            "  {} overengineering: {} (consumers={}): {}",
            ui::warn_badge(),
            ui::cyan(&w.file),
            w.consumer_count,
            w.reason
        );
    }
}

fn build_tier1_scopes(blast: &BlastRadiusReport) -> (Vec<String>, Vec<String>) {
    let mut tc_scope = blast.changed_files.clone();
    for item in &blast.impacted_files {
        if !tc_scope.contains(&item.file) {
            tc_scope.push(item.file.clone());
        }
    }
    let mut cache_scope = tc_scope.clone();
    for test in &blast.related_tests {
        if !cache_scope.contains(test) {
            cache_scope.push(test.clone());
        }
    }
    (tc_scope, cache_scope)
}

fn try_tier1_cache_hit(root: &Path, cache_key: Option<&str>) -> bool {
    let Some(key) = cache_key else {
        return false;
    };
    if matches!(check_cache::lookup(root, key), Ok(Some(entry)) if entry.passed) {
        println!("  {} {}", ui::cyan("[Tier 1]"), ui::green("PASS (cached)"));
        let _ = record_event(root, StatsEvent::CheckCacheHit);
        return true;
    }
    false
}

fn store_tier1_cache(root: &Path, cache_key: Option<&str>) {
    let Some(key) = cache_key else {
        return;
    };
    let entry = CachedTier1 {
        key: key.to_owned(),
        passed: true,
        summary: "Tier 1 PASS".to_owned(),
        recorded_at: Utc::now(),
    };
    let _ = check_cache::store(root, entry);
}

fn report_compressed(kind: &str, outputs: &[CompressedOutput]) -> bool {
    let mut failed = false;
    for out in outputs {
        let tag = ui::dim(&format!("[{kind}:{}] ", out.tool));
        if out.passed {
            println!("  {tag}{}", ui::green(&out.summary));
        } else {
            failed = true;
            println!("  {tag}{}", ui::red(&out.summary));
            for line in &out.lines {
                println!("    {}", ui::red(line));
            }
        }
    }
    failed
}

fn has_installed_typechecker(root: &Path, config: &AnvilConfig) -> bool {
    if has_typechecker_in_dir(root, &config.stacks) {
        return true;
    }
    config.subrepos.iter().any(|sub| {
        let sub_dir = root.join(&sub.path);
        let stacks = if sub.stacks.is_empty() {
            &config.stacks
        } else {
            &sub.stacks
        };
        sub_dir.is_dir() && has_typechecker_in_dir(&sub_dir, stacks)
    })
}

fn has_typechecker_in_dir(dir: &Path, stacks: &[StackKind]) -> bool {
    let needs_tsc = stacks.iter().any(|s| {
        matches!(
            s,
            StackKind::TypeScript
                | StackKind::React
                | StackKind::Vue
                | StackKind::Svelte
                | StackKind::Vite
                | StackKind::NextJs
                | StackKind::Astro
                | StackKind::Angular
                | StackKind::NestJs
                | StackKind::ExpoReactNative
        )
    });
    if needs_tsc
        && (resolve_local_or_path_bin(dir, "tsgo").is_some()
            || resolve_local_or_path_bin(dir, "tsc").is_some()
            || resolve_local_or_path_bin(dir, "vue-tsc").is_some())
    {
        return true;
    }
    if stacks.contains(&StackKind::Python) && resolve_local_or_path_bin(dir, "pyright").is_some() {
        return true;
    }
    if stacks.contains(&StackKind::FlutterDart)
        && resolve_local_or_path_bin(dir, "dart").is_some()
    {
        return true;
    }
    if stacks.contains(&StackKind::Rust) && resolve_local_or_path_bin(dir, "cargo").is_some() {
        return true;
    }
    if (stacks.contains(&StackKind::Php) || stacks.contains(&StackKind::Laravel))
        && has_php_analyzer(dir)
    {
        return true;
    }
    if stacks.contains(&StackKind::Godot) && resolve_godot_bin().is_some() {
        return true;
    }
    stacks.contains(&StackKind::KotlinAndroid) && has_gradle_bin(dir)
}

fn has_php_analyzer(dir: &Path) -> bool {
    resolve_php_vendor_script(dir, "phpstan").is_some()
        || resolve_php_vendor_script(dir, "psalm").is_some()
        || resolve_local_or_path_bin(dir, "phpstan").is_some()
        || resolve_local_or_path_bin(dir, "psalm").is_some()
        || resolve_php_bin().is_some()
}

fn has_gradle_bin(dir: &Path) -> bool {
    (cfg!(windows) && dir.join("gradlew.bat").is_file())
        || dir.join("gradlew").is_file()
        || resolve_local_or_path_bin(dir, "gradle").is_some()
}
