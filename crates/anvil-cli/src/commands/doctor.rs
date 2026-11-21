//! `anvil doctor`: audits config, adapters, strict TS, graph, memory, identity, circuit, and tools.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use anvil_adapters::{antigravity, claude_code};
use anvil_core::circuit_breaker::CircuitBreaker;
use anvil_core::config::{AnvilConfig, ToolKind};
use anvil_core::fs_util::{resolve_local_or_path_bin, strip_jsonc};
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
    check_stack_tools(root, &config);

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
        match resolve_tsconfig_strict(root, &ts_path, &mut visited, 0) {
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
    root: &Path,
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
            if let Some(ext_path) = resolve_extends_file(root, parent_dir, ext)
                && let Some(val) = resolve_tsconfig_strict(root, &ext_path, visited, depth + 1)?
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

fn resolve_extends_file(root: &Path, dir: &Path, ext: &str) -> Option<PathBuf> {
    let ext = ext.trim();
    if ext.is_empty() {
        return None;
    }
    if let Some(found) = probe_tsconfig_path(&dir.join(ext)) {
        return Some(found);
    }
    if ext.starts_with('.') || ext.starts_with('/') {
        return None;
    }
    let mut cur = Some(dir);
    while let Some(c) = cur {
        if let Some(found) = probe_tsconfig_path(&c.join("node_modules").join(ext)) {
            return Some(found);
        }
        if c == root {
            break;
        }
        cur = c.parent().filter(|p| p.starts_with(root));
    }
    None
}

fn probe_tsconfig_path(base: &Path) -> Option<PathBuf> {
    if base.is_file() {
        return Some(base.to_path_buf());
    }
    let mut json_os = base.as_os_str().to_os_string();
    json_os.push(".json");
    let with_json = PathBuf::from(json_os);
    if with_json.is_file() {
        return Some(with_json);
    }
    let dir_cfg = base.join("tsconfig.json");
    dir_cfg.is_file().then_some(dir_cfg)
}

fn check_graph_index(root: &Path) {
    let bin = store::default_bin_path(root);
    if !bin.exists() {
        println!("[WARN] graph: .harness/.state/graph.bin missing (run `anvil graph build`)");
        return;
    }
    match store::read_header(&bin) {
        Ok(header) => println!(
            "[OK]   graph: {} nodes, {} edges",
            header.live_nodes, header.live_edges
        ),
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

fn check_stack_tools(root: &Path, config: &AnvilConfig) {
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
        .filter(|b| resolve_local_or_path_bin(root, b).is_none())
        .collect();
    if missing.is_empty() {
        println!("[OK]   tools: all detected CLI tools found in local/PATH");
    } else {
        println!("[WARN] tools: not on local/PATH: {}", missing.join(", "));
    }
}
