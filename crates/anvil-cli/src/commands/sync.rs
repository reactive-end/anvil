//! `anvil sync`: non-destructive adapter and skill synchronization.

use std::path::Path;

use anyhow::Result;
use anvil_adapters::sync;

/// Synchronizes `.harness/skills/` and Antigravity / Claude Code adapter files.
pub fn execute(root: &Path) -> Result<()> {
    let report = sync::sync_repo(root)?;
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
