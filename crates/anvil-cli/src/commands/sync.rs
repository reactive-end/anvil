//! `anvil sync`: non-destructive adapter, config, and subproject synchronization.

use std::path::Path;

use anyhow::Result;
use anvil_adapters::sync;
use anvil_core::config::{AdapterTarget, AnvilConfig};

/// Synchronizes adapters, skills, and stack tools in `root` and initialized subprojects.
pub fn execute(root: &Path, adapter_raw: &str) -> Result<()> {
    let explicit_adapter = AdapterTarget::parse_cli(adapter_raw)?;
    let mut report = sync::sync_repo_with_adapter(root, explicit_adapter)?;
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

    println!(
        "anvil sync complete: {} files updated, {} pre-existing skills preserved",
        report.written_files.len(),
        report.preserved_skills.len()
    );
    for path in &report.written_files {
        println!("  updated: {}", path.display());
    }
    Ok(())
}
