//! `anvil sync`: non-destructive adapter, config, and subproject synchronization.

use std::path::Path;

use anvil_adapters::sync;
use anvil_core::config::{AdapterTarget, AnvilConfig};
use anyhow::Result;

use crate::ui;

/// Synchronizes adapters, skills, and stack tools in `root` and initialized subprojects.
pub fn execute(root: &Path, adapter_raw: &str) -> Result<()> {
    let explicit_adapter = AdapterTarget::parse_cli(adapter_raw)?;
    let mut report = sync::SyncReport::default();

    if !root.join(".harness").is_dir() {
        let umbrella_subs = sync::discover_umbrella_subrepos(root);
        if !umbrella_subs.is_empty() {
            let (_, umb_report) = sync::init_umbrella_root(root, explicit_adapter, &umbrella_subs)?;
            report.written_files.extend(umb_report.written_files);
            for sub in &umbrella_subs {
                let sub_report = sync::sync_repo_with_adapter(&sub.abs_path, explicit_adapter)?;
                report.written_files.extend(sub_report.written_files);
                report.preserved_skills.extend(sub_report.preserved_skills);
            }
            print_sync_summary(&report);
            return Ok(());
        }
    }

    report = sync::sync_repo_with_adapter(root, explicit_adapter)?;
    let cfg = AnvilConfig::load_or_default(root)?;

    for sub in &cfg.subrepos {
        if sub.path == "." {
            continue;
        }
        let sub_dir = root.join(&sub.path);
        if sub_dir.join(".harness").is_dir() {
            let sub_report = sync::sync_repo_with_adapter(&sub_dir, explicit_adapter)?;
            report.written_files.extend(sub_report.written_files);
            report.preserved_skills.extend(sub_report.preserved_skills);
        }
    }

    print_sync_summary(&report);
    Ok(())
}

fn print_sync_summary(report: &sync::SyncReport) {
    let prefix = if ui::colors_enabled() { "✓ " } else { "" };
    println!(
        "{}{} {} files updated, {} pre-existing skills preserved",
        ui::green(prefix),
        ui::green("anvil sync complete:"),
        ui::bold(&report.written_files.len().to_string()),
        report.preserved_skills.len()
    );
    if !report.written_files.is_empty() {
        ui::print_section("Updated Files");
        for path in &report.written_files {
            println!("  {} {}", ui::dim("updated:"), ui::cyan(&path.display().to_string()));
        }
    }
}
