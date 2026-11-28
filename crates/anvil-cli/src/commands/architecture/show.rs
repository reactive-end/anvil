//! `anvil architecture show`: styles, generated rules, and the guidance agents receive.

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_core::config::architecture::guidance::guidance;
use anvil_core::config::architecture::presets::expand;
use anvil_core::config::architecture::{ArchRule, ArchitectureStyle};
use anyhow::Result;
use serde::Serialize;

use crate::ui;

#[derive(Serialize)]
struct TargetReport {
    /// Subrepo name, or `null` for the repository root.
    subrepo: Option<String>,
    style: ArchitectureStyle,
    label: &'static str,
    source_dir: String,
    modules: Vec<String>,
    strict_framework: bool,
    guidance: &'static str,
    rules: Vec<ArchRule>,
}

#[derive(Serialize)]
struct Report {
    targets: Vec<TargetReport>,
    /// Number of the user's own `boundaries`, which the presets never touch.
    user_boundaries: usize,
}

fn build(config: &AnvilConfig) -> Report {
    let targets = config
        .architecture_all_targets()
        .iter()
        .map(|target| TargetReport {
            subrepo: target.subrepo.map(str::to_owned),
            style: target.setting.style,
            label: target.setting.style.label(),
            source_dir: target.source_dir(),
            modules: target.setting.modules.clone(),
            strict_framework: target.setting.strict_framework.unwrap_or(false),
            guidance: guidance(target.setting.style),
            rules: expand(target),
        })
        .collect();
    Report { targets, user_boundaries: config.boundaries.len() }
}

/// Prints the report as JSON or as a compact text summary.
pub(super) fn run(root: &Path, json: bool) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    let report = build(&config);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text(&report);
    }
    Ok(())
}

fn print_text(report: &Report) {
    let active = report.targets.iter().filter(|t| t.style != ArchitectureStyle::None);
    if active.clone().next().is_none() {
        println!("No architecture style configured.");
        println!("  suggest: anvil architecture detect");
        println!(
            "  record:  anvil architecture set <hexagonal|clean|layered|feature-sliced|modular-monolith|event-driven>"
        );
    }
    for target in active {
        let name = target.subrepo.as_deref().unwrap_or("this repository");
        println!("{}: {} ({})", ui::bold(name), target.style, target.label);
        ui::print_kv("source root", &target.source_dir);
        ui::print_kv("guidance", target.guidance);
        println!("  rules ({}):", target.rules.len());
        for rule in &target.rules {
            println!("    {}", ui::cyan(&rule.name));
            println!("      {}", rule.describe());
            println!("      {}", ui::dim(&format!("why: {}", rule.reason)));
        }
    }
    if report.user_boundaries > 0 {
        println!(
            "{} user boundary rule(s) in config.json are kept as they are.",
            report.user_boundaries
        );
    }
}
