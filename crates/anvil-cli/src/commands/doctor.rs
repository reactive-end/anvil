//! `anvil doctor`: audits config, adapters, strict TS, graph, memory, identity, circuit, and tools.

use std::fs;
use std::path::{Path, PathBuf};

use anvil_adapters::{antigravity, claude_code};
use anvil_core::circuit_breaker::CircuitBreaker;
use anvil_core::config::{AnvilConfig, StackKind, ToolKind};
use anvil_core::fs_util::{
    resolve_godot_bin, resolve_local_or_path_bin, resolve_php_bin, resolve_sonar_bin, strip_jsonc,
};
use anvil_graph::store;
use anvil_memory::MemoryEngine;
use anvil_tasks::resolve_author;
use anyhow::Result;
use rustc_hash::FxHashSet;
use serde_json::Value;

use crate::commands::ExitCodeError;
use crate::ui;

/// Runs repository health diagnostics and fails only on critical missing/broken setup.
pub fn execute(root: &Path) -> Result<()> {
    ui::print_banner_if_tty("Repository Health Diagnostics");
    ui::print_section("Health Checks");
    let mut failures = 0usize;
    let config = check_config(root, &mut failures);
    check_adapters(root, &config, &mut failures);
    check_tsconfig_strict(root, &config, &mut failures);
    check_phpstan_config(root, &config);
    check_godot_project(root, &config);
    check_sonar_setup(root, &config);
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
    if ui::colors_enabled() {
        println!();
    }
    println!("{}", ui::green("✓ doctor: all critical checks passed"));
    Ok(())
}

fn check_config(root: &Path, failures: &mut usize) -> AnvilConfig {
    match AnvilConfig::load(root) {
        Ok(cfg) => {
            ui::print_status_row(
                &ui::ok_badge(),
                "config",
                &format!(
                    ".harness/config.json  (stacks={}, subrepos={})",
                    ui::cyan(&cfg.stacks.len().to_string()),
                    ui::cyan(&cfg.subrepos.len().to_string())
                ),
            );
            cfg
        }
        Err(err) => {
            *failures += 1;
            ui::print_status_row(&ui::fail_badge(), "config", &err.to_string());
            AnvilConfig::default()
        }
    }
}

fn check_adapters(root: &Path, config: &AnvilConfig, failures: &mut usize) {
    let mut required = Vec::new();
    if config.adapters.includes_antigravity() {
        required.extend([
            antigravity::config::hooks_config_path(root),
            antigravity::config::mcp_config_path(root),
            root.join("AGENTS.md"),
        ]);
    }
    if config.adapters.includes_claude() {
        required.extend([
            claude_code::config::settings_config_path(root),
            claude_code::config::mcp_config_path(root),
            root.join("CLAUDE.md"),
        ]);
    }
    if config.adapters.includes_cursor() {
        required.extend([root.join(".cursor/mcp.json"), root.join(".cursor/rules/anvil.mdc")]);
    }
    if config.adapters.includes_windsurf() {
        required.extend([
            root.join(".windsurf/mcp_config.json"),
            root.join(".windsurf/rules/anvil.md"),
        ]);
    }
    if config.adapters.includes_codex() {
        required.extend([root.join(".codex/config.toml"), root.join("AGENTS.md")]);
    }
    let missing: Vec<_> = required.into_iter().filter(|p| !p.exists()).collect();
    if missing.is_empty() {
        ui::print_status_row(
            &ui::ok_badge(),
            "adapters",
            &format!("{:?} files present", config.adapters),
        );
    } else {
        *failures += 1;
        ui::print_status_row(
            &ui::fail_badge(),
            "adapters",
            &format!("missing {} file(s); run `anvil sync`", missing.len()),
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
                ui::print_status_row(
                    &ui::ok_badge(),
                    "tsconfig",
                    &format!("strict=true  {}", ui::dim(&format!("({})", ts_path.display()))),
                );
            }
            Ok(Some(false)) | Ok(None) => {
                *failures += 1;
                ui::print_status_row(
                    &ui::fail_badge(),
                    "tsconfig",
                    &format!("compilerOptions.strict is not true ({})", ts_path.display()),
                );
            }
            Err(()) => {
                ui::print_status_row(
                    &ui::warn_badge(),
                    "tsconfig",
                    &format!("could not parse {}", ts_path.display()),
                );
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
    if let Some(strict) =
        val.get("compilerOptions").and_then(|c| c.get("strict")).and_then(Value::as_bool)
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
    if ext.is_empty() || Path::new(ext).is_absolute() {
        return None;
    }
    let root_canon = root.canonicalize().ok()?;
    let is_contained = |cand: &Path| cand.canonicalize().is_ok_and(|c| c.starts_with(&root_canon));
    if let Some(found) = probe_tsconfig_path(&dir.join(ext)).filter(|p| is_contained(p)) {
        return Some(found);
    }
    if ext.starts_with('.') || ext.starts_with('/') {
        return None;
    }
    let mut cur = Some(dir);
    while let Some(c) = cur {
        if let Some(found) =
            probe_tsconfig_path(&c.join("node_modules").join(ext)).filter(|p| is_contained(p))
        {
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
        ui::print_status_row(
            &ui::warn_badge(),
            "graph",
            ".harness/.state/graph.bin missing (run `anvil graph build`)",
        );
        return;
    }
    match store::read_header(&bin) {
        Ok(header) => ui::print_status_row(
            &ui::ok_badge(),
            "graph",
            &format!(
                "{} nodes, {} edges",
                ui::cyan(&header.live_nodes.to_string()),
                ui::cyan(&header.live_edges.to_string())
            ),
        ),
        Err(err) => ui::print_status_row(
            &ui::warn_badge(),
            "graph",
            &format!("unreadable ({err}); run `anvil graph build`"),
        ),
    }
}

fn check_identity(root: &Path) -> String {
    match resolve_author(root) {
        Ok(author) if author != "unknown" => {
            ui::print_status_row(&ui::ok_badge(), "identity", &ui::cyan(&author));
            author
        }
        _ => {
            ui::print_status_row(
                &ui::warn_badge(),
                "identity",
                "could not resolve `gh` or `git user.name`",
            );
            "unknown".to_owned()
        }
    }
}

fn check_memory(root: &Path, author: &str) {
    match MemoryEngine::open_without_sync(root, author) {
        Ok(mut mem) => match mem.sync() {
            Ok(s) => ui::print_status_row(
                &ui::ok_badge(),
                "memory",
                &format!("SQLite FTS5 ready (synced {} new records)", s.records_applied),
            ),
            Err(err) => ui::print_status_row(
                &ui::warn_badge(),
                "memory",
                &format!("SQLite sync error: {err}"),
            ),
        },
        Err(err) => ui::print_status_row(&ui::warn_badge(), "memory", &err.to_string()),
    }
}

fn check_circuit(root: &Path, limit: u32) {
    let cb = CircuitBreaker::new(root, limit);
    match cb.status() {
        Ok(st) if st.is_open() => ui::print_status_row(
            &ui::warn_badge(),
            "circuit",
            &format!(
                "OPEN ({} consecutive failures; run `anvil circuit reset`)",
                st.consecutive_failures
            ),
        ),
        Ok(st) => ui::print_status_row(
            &ui::ok_badge(),
            "circuit",
            &format!("{:?} (streak={})", st.state, st.consecutive_failures),
        ),
        Err(err) => ui::print_status_row(&ui::warn_badge(), "circuit", &err.to_string()),
    }
}

fn check_stack_tools(root: &Path, config: &AnvilConfig) {
    let mut bins = vec!["git", "gh"];
    if workspace_has_php(config) {
        bins.push("php");
    }
    for tool in config.tools.iter().chain(config.subrepos.iter().flat_map(|s| s.tools.iter())) {
        if let Some(bin) = tool_kind_bin(*tool)
            && !bins.contains(&bin)
        {
            bins.push(bin);
        }
    }
    let search_dirs: Vec<PathBuf> = std::iter::once(root.to_path_buf())
        .chain(config.subrepos.iter().map(|s| root.join(&s.path)))
        .filter(|d| d.is_dir())
        .collect();
    let missing: Vec<&str> = bins
        .into_iter()
        .filter(|b| match *b {
            "tsc" => !search_dirs.iter().any(|d| {
                resolve_local_or_path_bin(d, "tsgo").is_some()
                    || resolve_local_or_path_bin(d, "tsc").is_some()
            }),
            "php" => resolve_php_bin().is_none(),
            "godot" => resolve_godot_bin().is_none(),
            other => !search_dirs.iter().any(|d| resolve_local_or_path_bin(d, other).is_some()),
        })
        .collect();
    if missing.is_empty() {
        ui::print_status_row(
            &ui::ok_badge(),
            "tools",
            "all detected CLI tools found in local/PATH",
        );
    } else {
        ui::print_status_row(
            &ui::warn_badge(),
            "tools",
            &format!("not on local/PATH: {}", ui::yellow(&missing.join(", "))),
        );
    }
}

fn workspace_has_php(config: &AnvilConfig) -> bool {
    config
        .stacks
        .iter()
        .chain(config.subrepos.iter().flat_map(|s| s.stacks.iter()))
        .any(|s| matches!(s, StackKind::Php | StackKind::Laravel))
}

fn tool_kind_bin(tool: ToolKind) -> Option<&'static str> {
    match tool {
        ToolKind::Tsc => Some("tsc"),
        ToolKind::VueTsc => Some("vue-tsc"),
        ToolKind::Eslint => Some("eslint"),
        ToolKind::Biome => Some("biome"),
        ToolKind::Ruff => Some("ruff"),
        ToolKind::Pyright => Some("pyright"),
        ToolKind::Mypy => Some("mypy"),
        ToolKind::Ktlint => Some("ktlint"),
        ToolKind::Detekt => Some("detekt"),
        ToolKind::DartAnalyze => Some("dart"),
        ToolKind::Vitest => Some("vitest"),
        ToolKind::Jest => Some("jest"),
        ToolKind::Pytest => Some("pytest"),
        ToolKind::Cargo => Some("cargo"),
        ToolKind::Composer => Some("composer"),
        ToolKind::PhpStan => Some("phpstan"),
        ToolKind::Psalm => Some("psalm"),
        ToolKind::Pint => Some("pint"),
        ToolKind::PhpCsFixer => Some("php-cs-fixer"),
        ToolKind::PhpUnit => Some("phpunit"),
        ToolKind::Pest => Some("pest"),
        ToolKind::Godot => Some("godot"),
        ToolKind::GdLint => Some("gdlint"),
        ToolKind::Gradle
        | ToolKind::Roborazzi
        | ToolKind::SonarQube
        | ToolKind::GdUnit4
        | ToolKind::Gut => None,
    }
}

fn check_sonar_setup(root: &Path, config: &AnvilConfig) {
    let any_sonar = config.sonar.enabled || config.subrepos.iter().any(|s| s.sonar_enabled);
    if !any_sonar {
        return;
    }
    if let Some(bin) = resolve_sonar_bin() {
        ui::print_status_row(
            &ui::ok_badge(),
            "sonar",
            &format!("scanner ready ({})", bin.display()),
        );
    } else {
        ui::print_status_row(
            &ui::warn_badge(),
            "sonar",
            "scanner binary (`sonar` / `sonar-scanner`) not found on PATH",
        );
    }
    let (host, project, token) =
        crate::commands::sonar::resolve_sonar_endpoint(root, config, None, None);
    let token_state = if token.is_some() { "token set" } else { "token unset" };
    let badge = if project.is_some() { ui::ok_badge() } else { ui::warn_badge() };
    ui::print_status_row(
        &badge,
        "sonar",
        &format!(
            "host={} project={} ({}: {token_state})",
            host.as_deref().unwrap_or("unset"),
            project.as_deref().unwrap_or("missing"),
            config.sonar.token_env
        ),
    );
}

fn check_godot_project(root: &Path, config: &AnvilConfig) {
    let has_godot = config.stacks.contains(&StackKind::Godot)
        || config.subrepos.iter().any(|s| s.stacks.contains(&StackKind::Godot));
    if !has_godot {
        return;
    }
    if resolve_godot_bin().is_some() {
        ui::print_status_row(&ui::ok_badge(), "godot", "headless Godot binary detected");
    } else {
        ui::print_status_row(
            &ui::warn_badge(),
            "godot",
            "Godot binary not found (`GODOT_BIN` / PATH); Tier 1 `--check-only` & `qa` paused",
        );
    }
    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
    if !gitignore.lines().any(|l| l.trim().starts_with(".godot")) {
        ui::print_status_row(
            &ui::warn_badge(),
            "godot",
            "add `.godot/` to `.gitignore` so imported engine caches stay untracked",
        );
    }
}

fn check_phpstan_config(root: &Path, config: &AnvilConfig) {
    let mut php_dirs = Vec::new();
    if config.stacks.iter().any(|s| matches!(s, StackKind::Php | StackKind::Laravel))
        && root.join("composer.json").is_file()
    {
        php_dirs.push(root.to_path_buf());
    }
    for sub in &config.subrepos {
        if sub.stacks.iter().any(|s| matches!(s, StackKind::Php | StackKind::Laravel)) {
            let dir = root.join(&sub.path);
            if dir.is_dir() && !php_dirs.contains(&dir) {
                php_dirs.push(dir);
            }
        }
    }
    for dir in php_dirs {
        let neon = ["phpstan.neon", "phpstan.neon.dist"]
            .into_iter()
            .map(|n| dir.join(n))
            .find(|p| p.is_file());
        if let Some(path) = neon {
            let level = parse_phpstan_level(&path);
            match level {
                Some(lvl) => ui::print_status_row(
                    &ui::ok_badge(),
                    "phpstan",
                    &format!(
                        "level={}  {}",
                        ui::cyan(&lvl),
                        ui::dim(&format!("({})", path.display()))
                    ),
                ),
                None => ui::print_status_row(
                    &ui::warn_badge(),
                    "phpstan",
                    &format!("no explicit level found ({})", path.display()),
                ),
            }
        } else if dir.join("psalm.xml").is_file() || dir.join("psalm.xml.dist").is_file() {
            ui::print_status_row(
                &ui::ok_badge(),
                "psalm",
                &format!("config present  {}", ui::dim(&format!("({})", dir.display()))),
            );
        } else {
            ui::print_status_row(
                &ui::warn_badge(),
                "phpstan",
                &format!(
                    "missing phpstan.neon/psalm.xml in {} (recommended for PHP/Laravel Tier 1)",
                    dir.display()
                ),
            );
        }
    }
}

fn parse_phpstan_level(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("level:") {
            let val = rest.split('#').next().unwrap_or("").trim();
            if !val.is_empty() {
                return Some(val.to_owned());
            }
        }
    }
    None
}
