//! `anvil doctor`: audits config, adapters, strict TS, graph, memory, identity, circuit, and tools.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use anvil_adapters::{antigravity, claude_code};
use anvil_core::circuit_breaker::CircuitBreaker;
use anvil_core::config::{AnvilConfig, ToolKind};
use anvil_core::fs_util::{bin_in_path, strip_jsonc};
use anvil_graph::store;
use anvil_memory::MemoryEngine;
use anvil_tasks::resolve_author;
use rustc_hash::FxHashSet;
use serde_json::Value;

use crate::commands::ExitCodeError;

/// Runs repository health diagnostics and fails only on critical missing/broken setup.
pub fn execute(root: &Path) -> Result<()> {
    let mut failures = 0usize;
    let config = check_config(root, &mut failures);
    check_adapters(root, &mut failures);
    check_tsconfig_strict(root, &config, &mut failures);
    check_graph_index(root);
    let author = check_identity(root);
    check_memory(root, &author);
    check_circuit(root, config.circuit_breaker_limit);
    check_stack_tools(&config);

    if failures > 0 {
        return Err(ExitCodeError::with_message(
            1,
            format!("doctor found {failures} critical issue(s)"),
        )
        .into());
    }
    println!("doctor: all critical checks passed");
    Ok(())
}

fn check_config(root: &Path, failures: &mut usize) -> AnvilConfig {
    match AnvilConfig::load(root) {
        Ok(cfg) => {
            println!("[OK]   config: .harness/config.json (stacks={})", cfg.stacks.len());
            cfg
        }
        Err(err) => {
            *failures += 1;
            println!("[FAIL] config: {err}");
            AnvilConfig::default()
        }
    }
}

fn check_adapters(root: &Path, failures: &mut usize) {
    let required = [
        antigravity::config::hooks_config_path(root),
        antigravity::config::mcp_config_path(root),
        claude_code::config::settings_config_path(root),
        claude_code::config::mcp_config_path(root),
        root.join("AGENTS.md"),
        root.join("CLAUDE.md"),
    ];
    let missing: Vec<_> = required.into_iter().filter(|p| !p.exists()).collect();
    if missing.is_empty() {
        println!("[OK]   adapters: Antigravity + Claude Code files present");
    } else {
        *failures += 1;
        println!(
            "[FAIL] adapters: missing {} file(s); run `anvil sync`",
            missing.len()
        );
    }
}

fn check_tsconfig_strict(root: &Path, config: &AnvilConfig, failures: &mut usize) {
    let mut candidates = vec![root.join("tsconfig.json")];
    for sub in &config.subrepos {
        candidates.push(root.join(&sub.path).join("tsconfig.json"));
    }
    for ts_path in candidates.into_iter().filter(|p| p.exists()) {
        let mut visited = FxHashSet::default();
        match resolve_tsconfig_strict(&ts_path, &mut visited, 0) {
            Ok(Some(true)) => {
                println!("[OK]   tsconfig: strict=true ({})", ts_path.display());
            }
            Ok(Some(false)) | Ok(None) => {
                *failures += 1;
                println!(
                    "[FAIL] tsconfig: compilerOptions.strict is not true ({})",
                    ts_path.display()
                );
            }
            Err(()) => {
                println!("[WARN] tsconfig: could not parse {}", ts_path.display());
            }
        }
    }
}

fn resolve_tsconfig_strict(
    ts_path: &Path,
    visited: &mut FxHashSet<PathBuf>,
    depth: usize,
) -> std::result::Result<Option<bool>, ()> {
    if depth > 8 || !visited.insert(ts_path.to_path_buf()) {
        return Ok(None);
    }
    let Ok(raw) = fs::read_to_string(ts_path) else {
        return Ok(None);
    };
    let cleaned = strip_jsonc(&raw);
    let val: Value = serde_json::from_str(&cleaned).map_err(|_| ())?;
    if let Some(strict) = val
        .get("compilerOptions")
        .and_then(|c| c.get("strict"))
        .and_then(Value::as_bool)
    {
        return Ok(Some(strict));
    }

    let mut inherited = None;
    if let Some(parent_dir) = ts_path.parent() {
        for ext in extract_extends_list(&val) {
            if let Some(ext_path) = resolve_extends_file(parent_dir, ext)
                && let Some(val) = resolve_tsconfig_strict(&ext_path, visited, depth + 1)?
            {
                inherited = Some(val);
            }
        }
    }
    Ok(inherited)
}

fn extract_extends_list(val: &Value) -> Vec<&str> {
    match val.get("extends") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(arr)) => arr.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

fn resolve_extends_file(dir: &Path, ext: &str) -> Option<PathBuf> {
    let direct = dir.join(ext);
    if direct.is_file() {
        return Some(direct);
    }
    let with_json = dir.join(format!("{ext}.json"));
    if with_json.is_file() {
        return Some(with_json);
    }
    let dir_cfg = direct.join("tsconfig.json");
    dir_cfg.is_file().then_some(dir_cfg)
}

fn check_graph_index(root: &Path) {
    let bin = store::default_bin_path(root);
    if !bin.exists() {
        println!("[WARN] graph: .harness/.state/graph.bin missing (run `anvil graph build`)");
        return;
    }
    match store::load_binary(&bin) {
        Ok(g) => println!("[OK]   graph: {} nodes, {} edges", g.nodes.len(), g.edges.len()),
        Err(err) => println!("[WARN] graph: unreadable ({err}); run `anvil graph build`"),
    }
}

fn check_identity(root: &Path) -> String {
    match resolve_author(root) {
        Ok(author) if author != "unknown" => {
            println!("[OK]   identity: {author}");
            author
        }
        _ => {
            println!("[WARN] identity: could not resolve `gh` or `git user.name`");
            "unknown".to_owned()
        }
    }
}

fn check_memory(root: &Path, author: &str) {
    match MemoryEngine::open_without_sync(root, author) {
        Ok(mut mem) => {
            let synced = mem.sync().map(|s| s.records_applied).unwrap_or(0);
            println!("[OK]   memory: SQLite FTS5 ready (synced {synced} new records)");
        }
        Err(err) => println!("[WARN] memory: {err}"),
    }
}

fn check_circuit(root: &Path, limit: u32) {
    let cb = CircuitBreaker::new(root, limit);
    match cb.status() {
        Ok(st) if st.is_open() => {
            println!(
                "[WARN] circuit: OPEN ({} consecutive failures; run `anvil circuit reset`)",
                st.consecutive_failures
            );
        }
        Ok(st) => println!("[OK]   circuit: {:?} (streak={})", st.state, st.consecutive_failures),
        Err(err) => println!("[WARN] circuit: {err}"),
    }
}

fn check_stack_tools(config: &AnvilConfig) {
    let mut bins = vec!["git", "gh"];
    for tool in &config.tools {
        let bin = match tool {
            ToolKind::Tsc => "tsc",
            ToolKind::Eslint => "eslint",
            ToolKind::Biome => "biome",
            ToolKind::Ruff => "ruff",
            ToolKind::Pyright => "pyright",
            ToolKind::Mypy => "mypy",
            ToolKind::Ktlint => "ktlint",
            ToolKind::Detekt => "detekt",
            ToolKind::DartAnalyze => "dart",
            ToolKind::Vitest => "vitest",
            ToolKind::Jest => "jest",
            ToolKind::Pytest => "pytest",
            ToolKind::Roborazzi | ToolKind::SonarQube => continue,
        };
        if !bins.contains(&bin) {
            bins.push(bin);
        }
    }
    let missing: Vec<&str> = bins
        .into_iter()
        .filter(|b| bin_in_path(b).is_none())
        .collect();
    if missing.is_empty() {
        println!("[OK]   tools: all detected CLI tools found in PATH");
    } else {
        println!("[WARN] tools: not on PATH: {}", missing.join(", "));
    }
}
