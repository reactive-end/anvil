//! `anvil init`: single-repo and multi-subproject `.harness/`, adapter, git, graph, and task setup.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use anvil_adapters::sync;
use anvil_core::config::{AdapterTarget, AnvilConfig};
use anvil_core::stack_detector;
use anvil_graph::CodeGraphEngine;
use anvil_tasks::{build_root_tasks_json, install_git_merge_driver};

/// Initializes `anvil` in `root` and any independent child project folders inside `root`.
pub fn execute(root: &Path, adapter_raw: &str) -> Result<()> {
    fs::create_dir_all(root)
        .with_context(|| format!("failed to create target directory {}", root.display()))?;

    let explicit_adapter = AdapterTarget::parse_cli(adapter_raw)?;
    let root_config = init_single_repo(root, explicit_adapter, "anvil initialized at")?;

    let detection = stack_detector::detect_workspace(root)?;
    let root_has_git = root.join(".git").exists();
    for repo in detection.repos.into_iter().filter(|r| r.rel_path != ".") {
        let sub_dir = root.join(&repo.rel_path);
        if should_init_subrepo(&sub_dir, root_has_git) {
            init_single_repo(
                &sub_dir,
                Some(root_config.adapters),
                &format!("  - subproject {} initialized at", repo.rel_path),
            )?;
        }
    }
    Ok(())
}

fn should_init_subrepo(sub_dir: &Path, root_has_git: bool) -> bool {
    sub_dir.is_dir() && (sub_dir.join(".git").exists() || !root_has_git)
}

fn init_single_repo(
    dir: &Path,
    explicit_adapter: Option<AdapterTarget>,
    banner: &str,
) -> Result<AnvilConfig> {
    let report = sync::init_repo_with_adapter(dir, explicit_adapter)?;
    let config = AnvilConfig::load_or_default(dir)?;

    let merge_driver_status = if dir.join(".git").exists() {
        install_git_merge_driver(dir)?;
        "installed"
    } else {
        "skipped (no .git)"
    };

    let engine = CodeGraphEngine::build(dir, config.clone())?;
    let graph_bin = engine.save()?;
    let tasks = build_root_tasks_json(dir)?;

    println!("{banner} {}", dir.display());
    println!(
        "  stacks={:?} tools={:?} adapters={:?} merge_driver={merge_driver_status} graph=({} nodes, {} edges) tasks={} files_written={} ({})",
        config.stacks,
        config.tools,
        config.adapters,
        engine.graph.nodes.len(),
        engine.graph.edges.len(),
        tasks.task_count,
        report.written_files.len(),
        graph_bin.display()
    );
    Ok(config)
}
