//! `anvil init`: single-repo and multi-subproject `.harness/`, adapter, git, graph, and task setup.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use anvil_adapters::sync;
use anvil_core::config::{AdapterTarget, AnvilConfig};
use anvil_core::stack_detector;
use anvil_graph::CodeGraphEngine;
use anvil_tasks::{build_root_tasks_json, install_git_merge_driver};

use crate::ui;

/// Initializes `anvil` in `root` and any independent child project folders inside `root`.
pub fn execute(root: &Path, adapter_raw: &str, git_hooks: bool) -> Result<()> {
    ui::print_banner_if_tty("Workspace Initialization");
    fs::create_dir_all(root)
        .with_context(|| format!("failed to create target directory {}", root.display()))?;

    let explicit_adapter = AdapterTarget::parse_cli(adapter_raw)?;
    let root_config = init_single_repo(root, explicit_adapter, git_hooks, "anvil initialized at")?;

    let detection = stack_detector::detect_workspace(root)?;
    let root_has_git = root.join(".git").exists();
    for repo in detection.repos.into_iter().filter(|r| r.rel_path != ".") {
        let sub_dir = root.join(&repo.rel_path);
        if should_init_subrepo(&sub_dir, root_has_git) {
            init_single_repo(
                &sub_dir,
                Some(root_config.adapters),
                git_hooks,
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
    git_hooks: bool,
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
    let pre_commit_status = if git_hooks {
        if crate::commands::hook::git_install::install_pre_commit_hook(dir)? {
            "installed"
        } else {
            "unchanged/skipped"
        }
    } else {
        "not requested"
    };

    let engine = CodeGraphEngine::build(dir, config.clone())?;
    let graph_bin = engine.save()?;
    let tasks = build_root_tasks_json(dir)?;

    let stacks_str = format_debug_list(&config.stacks);
    let tools_str = format_debug_list(&config.tools);
    let prefix = if ui::colors_enabled() { "✓ " } else { "" };

    println!(
        "{}{} {}",
        ui::green(prefix),
        ui::green(banner),
        ui::bold(&dir.display().to_string())
    );
    ui::print_kv("stacks", &ui::cyan(&stacks_str));
    ui::print_kv("tools", &ui::cyan(&tools_str));
    ui::print_kv("adapters", &format!("{:?}", config.adapters));
    ui::print_kv("merge_driver", merge_driver_status);
    if git_hooks {
        ui::print_kv("pre_commit_hook", pre_commit_status);
    }
    ui::print_kv(
        "graph",
        &format!(
            "{} nodes, {} edges  {}",
            ui::bold(&engine.graph.nodes.len().to_string()),
            ui::bold(&engine.graph.edges.len().to_string()),
            ui::dim(&format!("({})", graph_bin.display()))
        ),
    );
    ui::print_kv(
        "summary",
        &format!(
            "tasks={}  files_written={}",
            tasks.task_count,
            report.written_files.len()
        ),
    );
    if ui::colors_enabled() {
        println!();
    }
    Ok(config)
}

fn format_debug_list<T: std::fmt::Debug>(items: &[T]) -> String {
    if items.is_empty() {
        return "none".to_owned();
    }
    items
        .iter()
        .map(|item| format!("{item:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}
