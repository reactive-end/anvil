//! `anvil init`: repository detection, `.harness/` scaffolding, adapters, merge driver, graph, and tasks.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use anvil_adapters::sync;
use anvil_core::config::AnvilConfig;
use anvil_graph::CodeGraphEngine;
use anvil_tasks::{build_root_tasks_json, install_git_merge_driver};

/// Initializes `anvil` in `root`.
pub fn execute(root: &Path) -> Result<()> {
    fs::create_dir_all(root)
        .with_context(|| format!("failed to create target directory {}", root.display()))?;

    let report = sync::init_repo(root)?;
    let config = AnvilConfig::load_or_default(root)?;

    let merge_driver_status = if root.join(".git").exists() {
        install_git_merge_driver(root)?;
        "installed"
    } else {
        "skipped (no .git)"
    };

    let engine = CodeGraphEngine::build(root, config.clone())?;
    let graph_bin = engine.save()?;
    let tasks = build_root_tasks_json(root)?;

    println!("anvil initialized at {}", root.display());
    println!(
        "- stacks: {:?} | tools: {:?}",
        config.stacks, config.tools
    );
    println!(
        "- adapters: {} dirs created, {} files written, {} skills preserved",
        report.created_dirs.len(),
        report.written_files.len(),
        report.preserved_skills.len()
    );
    println!("- git merge driver: {merge_driver_status}");
    println!(
        "- code graph: {} nodes, {} edges ({})",
        engine.graph.nodes.len(),
        engine.graph.edges.len(),
        graph_bin.display()
    );
    println!("- tasks.json: {} tasks", tasks.task_count);
    Ok(())
}
