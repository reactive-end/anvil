//! `anvil check`: diff-classified Tier 0 / Tier 1 / Tier 2 verification.

use std::fs;
use std::path::Path;

use anyhow::Result;
use anvil_ast::analyze_file;
use anvil_core::config::{AnvilConfig, StackKind};
use anvil_core::diff_classifier::{ChangedFile, DiffClassification, classify_files};
use anvil_core::git::{self, DiffScope};
use anvil_core::traits::{Severity, Tier};
use anvil_deps::{AuditOptions, LiveRegistryFetcher, audit_changed_files};
use anvil_graph::CodeGraphEngine;
use anvil_graph::builder::collect_source_files;
use anvil_graph::model::CodeGraph;
use anvil_qa::QaEngine;

use crate::commands::ExitCodeError;

/// Executes `anvil check [--full] [--tier 0|1|2]`.
pub fn execute(root: &Path, full: bool, tier_override: Option<u8>) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    let (changed_paths, classification) = classify_repo_diff(root);
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
        tier1_failed = run_tier1(root, &config, &changed_paths, &classification)?;
    }
    if effective_tier >= Tier::Tier2 {
        run_tier2(root, &config, &changed_paths, &classification, full);
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

fn classify_repo_diff(root: &Path) -> (Vec<String>, DiffClassification) {
    let paths = git::changed_files(root, DiffScope::WorkingTree).unwrap_or_default();
    let changed: Vec<ChangedFile> = paths
        .iter()
        .map(|p| match git::file_diff(root, p, false) {
            Ok(hunks) if !hunks.is_empty() => ChangedFile::with_hunks(p, hunks),
            _ => ChangedFile::from_path(p),
        })
        .collect();
    let classification = classify_files(&changed);
    (paths, classification)
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

    let mut errors = 0usize;
    for rel in targets {
        let abs = root.join(&rel);
        if !abs.is_file() {
            continue;
        }
        let Ok(source) = fs::read_to_string(&abs) else {
            continue;
        };
        let facts = analyze_file(Path::new(&rel), &source, config)?;
        for diag in facts.diagnostics {
            let tag = match diag.severity {
                Severity::Error => {
                    errors += 1;
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
    Ok(errors)
}

fn run_tier1(
    root: &Path,
    config: &AnvilConfig,
    changed: &[String],
    classification: &DiffClassification,
) -> Result<bool> {
    let engine = CodeGraphEngine::load_or_build(root, config.clone())?;
    let blast = engine.blast_radius(changed, 2);
    println!(
        "  [Tier 1] blast_radius: {} impacted, {} related test(s) ({:.2}ms)",
        blast.impacted_files.len(),
        blast.related_tests.len(),
        blast.elapsed_ms
    );

    let added: Vec<String> = changed
        .iter()
        .filter(|p| {
            let norm = CodeGraph::normalize_rel_path(p);
            !engine.graph.file_hashes.contains_key(&norm)
        })
        .cloned()
        .collect();
    let over = engine.check_overengineering(changed, &added, changed.len() <= 3);
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

    if !has_installed_typechecker(config) {
        return Ok(false);
    }
    let mut scope = blast.changed_files;
    for item in blast.impacted_files {
        if !scope.contains(&item.file) {
            scope.push(item.file);
        }
    }
    let mut failed = false;
    for out in engine.run_impacted_typecheck(&scope)? {
        println!("  [typecheck:{}] {}", out.tool, out.summary);
        if !out.passed {
            failed = true;
            for line in out.lines {
                println!("    {line}");
            }
        }
    }
    Ok(failed)
}

fn run_tier2(
    root: &Path,
    config: &AnvilConfig,
    changed: &[String],
    classification: &DiffClassification,
    full: bool,
) {
    if classification.needs_deps_audit || full {
        let paths: Vec<_> = changed.iter().map(std::path::PathBuf::from).collect();
        if let Ok(options) = AuditOptions::from_repo(root)
            && let Ok(report) = audit_changed_files(root, &paths, &LiveRegistryFetcher, &options)
            && !report.findings.is_empty()
        {
            println!("  [Tier 2] deps audit: {} finding(s)", report.findings.len());
        }
    }
    if classification.needs_visual_qa || full {
        let qa = QaEngine::new(root);
        if let Some(prompt) = qa.onboarding_prompt(config) {
            println!(
                "  [Tier 2] visual QA driver unconfigured for {:?}; run `anvil qa onboard`",
                prompt.platform
            );
        } else {
            println!("  [Tier 2] visual QA platform={:?}", qa.detect_platform(config));
        }
    }
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
    if needs_tsc && cmd_exists("tsc") {
        return true;
    }
    if config.stacks.contains(&StackKind::Python) && cmd_exists("pyright") {
        return true;
    }
    config.stacks.contains(&StackKind::FlutterDart) && cmd_exists("dart")
}

fn cmd_exists(bin: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    let exts = if cfg!(windows) {
        [".EXE", ".CMD", ".BAT", ""]
    } else {
        ["", "", "", ""]
    };
    std::env::split_paths(&path_var).any(|dir| {
        exts.iter()
            .any(|ext| dir.join(format!("{bin}{ext}")).is_file())
    })
}
