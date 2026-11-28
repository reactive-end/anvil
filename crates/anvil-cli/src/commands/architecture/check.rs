//! `anvil architecture check`: every import in the repository against the generated rules.

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_core::config::architecture::rules::violation_target;
use anvil_graph::CodeGraphEngine;
use anyhow::Result;
use serde::Serialize;

use crate::commands::ExitCodeError;
use crate::ui;

/// Prefix of the diagnostics produced by generated architecture rules.
const ARCH_RULE_PREFIX: &str = "boundary:arch:";

#[derive(Serialize)]
struct Violation {
    rule: String,
    file: String,
    line: u32,
    target: String,
    reason: String,
}

#[derive(Serialize)]
struct Report {
    ok: bool,
    styles: Vec<String>,
    violations: Vec<Violation>,
}

/// Checks the whole repository; exits 1 when any import breaks a generated rule.
pub(super) fn run(root: &Path, json: bool) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    let styles: Vec<String> = config
        .architecture_targets()
        .iter()
        .map(|t| format!("{} ({})", t.setting.style, t.label()))
        .collect();
    let violations = if styles.is_empty() { Vec::new() } else { find_violations(root, config)? };
    let report = Report { ok: violations.is_empty(), styles, violations };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text(&report);
    }
    if report.ok { Ok(()) } else { Err(ExitCodeError::silent(1).into()) }
}

fn find_violations(root: &Path, config: AnvilConfig) -> Result<Vec<Violation>> {
    let engine = CodeGraphEngine::load_or_build(root, config)?;
    let mut violations: Vec<Violation> = engine
        .check_boundaries(&[])
        .into_iter()
        .filter(|d| d.rule.starts_with(ARCH_RULE_PREFIX))
        .map(|d| Violation {
            rule: d.rule.trim_start_matches("boundary:").to_owned(),
            target: violation_target(&d.message).unwrap_or(&d.message).to_owned(),
            reason: d.suggestion.unwrap_or_default(),
            file: d.file,
            line: d.line,
        })
        .collect();
    violations.sort_by(|a, b| (&a.file, a.line, &a.rule).cmp(&(&b.file, b.line, &b.rule)));
    Ok(violations)
}

fn print_text(report: &Report) {
    if report.styles.is_empty() {
        println!("No architecture style configured; nothing to check.");
        println!(
            "  record one with: anvil architecture set <style>   (see `anvil architecture detect`)"
        );
        return;
    }
    for v in &report.violations {
        println!("{} {}:{} -> {}", ui::error_badge(), ui::bold(&v.file), v.line, v.target);
        println!("    {}", ui::cyan(&v.rule));
        println!("    {}", ui::dim(&v.reason));
    }
    let styles = report.styles.join(", ");
    if report.ok {
        println!("{} architecture ok: {styles}", ui::green("✓"));
    } else {
        println!("{} architecture violation(s) against {styles}", report.violations.len());
    }
}
