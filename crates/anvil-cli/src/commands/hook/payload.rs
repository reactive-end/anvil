//! Hook stdin/tool-input extraction and tool classification helpers for `anvil hook`.

use std::path::{Path, PathBuf};

use anvil_core::fs_util::relativize_path;
use anvil_core::git;
use anvil_graph::model::CodeGraph;
use serde_json::Value;

const SHELL_METACHARS: &[&str] = &["&&", "||", "|", ";", ">", "<", "$(", "`", "\n", "\r"];
const FILE_PATH_KEYS: &[&str] = &[
    "Path",
    "TargetFile",
    "file_path",
    "path",
    "AbsolutePath",
    "target_file",
    "filePath",
    "FilePath",
];

/// Resolves `(effective_root, fallback_workspace_root)` for a hook invocation.
/// When a tool payload targets a file inside an initialized subproject or external repository
/// (e.g. `centinela-ia-movil` while the IDE is open in `centinela/`), hooks route directly to
/// that target project while remembering the umbrella workspace for pending prompt handoff.
pub fn resolve_hook_targets(
    default_root: &Path,
    cwd: Option<&str>,
    tool_input: Option<&Value>,
) -> (PathBuf, Option<PathBuf>) {
    let cwd_root = resolve_hook_root(default_root, cwd);
    if let Some(target_file) = extract_target_file_path(tool_input, &cwd_root)
        && let Some(start_dir) = target_file.parent().filter(|p| p.exists())
        && let Some(file_root) = find_initialized_root_upward(start_dir)
        && !paths_equal(&file_root, &cwd_root)
    {
        return (file_root, Some(cwd_root));
    }
    (cwd_root, None)
}

/// Resolves the effective repository root from a hook's `cwd` field, supporting non-git umbrella
/// workspaces that hold `.harness/`.
pub fn resolve_hook_root(default_root: &Path, cwd: Option<&str>) -> PathBuf {
    let Some(raw) = cwd.map(str::trim).filter(|s| !s.is_empty()) else {
        return default_root.to_path_buf();
    };
    let path = PathBuf::from(raw);
    if !path.is_dir() {
        return default_root.to_path_buf();
    }
    if path.join(".harness").is_dir() {
        return path;
    }
    find_initialized_root_upward(&path)
        .or_else(|| git::repo_root(&path).ok())
        .unwrap_or(path)
}

fn find_initialized_root_upward(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(".harness").is_dir())
        .map(Path::to_path_buf)
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

fn extract_raw_file_path(tool_input: Option<&Value>) -> Option<&str> {
    let obj = tool_input?.as_object()?;
    for key in FILE_PATH_KEYS {
        if let Some(val) = obj.get(*key).and_then(Value::as_str) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    for (k, v) in obj {
        if let Some(val) = v.as_str()
            && FILE_PATH_KEYS.iter().any(|cand| cand.eq_ignore_ascii_case(k))
        {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

fn extract_target_file_path(tool_input: Option<&Value>, cwd_root: &Path) -> Option<PathBuf> {
    let raw = extract_raw_file_path(tool_input)?;
    let p = PathBuf::from(raw);
    if p.is_absolute() {
        Some(p)
    } else {
        Some(cwd_root.join(p))
    }
}

/// Checks loop-guard continuation flags across Antigravity and Claude Code `Stop` payloads.
pub fn is_continuation_active(raw_json: &str) -> bool {
    let Ok(val) = serde_json::from_str::<Value>(raw_json) else {
        return false;
    };
    ["continuationActive", "continuation_active", "stop_hook_active"]
        .iter()
        .any(|k| val.get(*k).and_then(Value::as_bool) == Some(true))
}

/// Returns `true` when `Stop` should immediately allow without running blocking gates
/// (loop guards, Antigravity `TERMINAL_STEP_TYPE`, or non-idle `fullyIdle == false` turns).
pub fn should_bypass_stop(raw_json: &str) -> bool {
    if is_continuation_active(raw_json) {
        return true;
    }
    let Ok(val) = serde_json::from_str::<Value>(raw_json) else {
        return false;
    };
    val.get("terminationReason").and_then(Value::as_str) == Some("TERMINAL_STEP_TYPE")
        || val.get("fullyIdle").and_then(Value::as_bool) == Some(false)
}

/// Extracts `(field_key, trimmed_command)` from a `PreToolUse` / `PostToolUse` `tool_input`.
pub fn extract_command_field(tool_input: Option<&Value>) -> Option<(&'static str, &str)> {
    let obj = tool_input?.as_object()?;
    ["CommandLine", "command", "cmd"]
        .into_iter()
        .find_map(|k| obj.get(k).and_then(Value::as_str).map(|s| (k, s.trim())))
}

/// Returns `true` if a command contains shell chaining, piping, or redirection metacharacters.
pub fn contains_shell_metacharacters(cmd: &str) -> bool {
    SHELL_METACHARS.iter().any(|meta| cmd.contains(meta))
}

/// Returns `true` if a simple shell command should be transparently wrapped with `anvil run --`.
pub fn should_wrap_with_anvil_run(cmd: &str) -> bool {
    if cmd.starts_with("anvil ") || cmd.contains("anvil run") || contains_shell_metacharacters(cmd)
    {
        return false;
    }
    [
        "tsc",
        "npx tsc",
        "eslint",
        "npx eslint",
        "biome",
        "vitest",
        "npx vitest",
        "jest",
        "npx jest",
        "pytest",
        "ruff",
        "dart analyze",
        "flutter test",
        "./gradlew",
        "gradlew",
        "phpstan",
        "vendor/bin/phpstan",
        "psalm",
        "vendor/bin/psalm",
        "phpunit",
        "vendor/bin/phpunit",
        "pest",
        "vendor/bin/pest",
        "pint",
        "vendor/bin/pint",
        "php artisan test",
        "cargo test",
        "cargo check",
        "cargo clippy",
        "cargo build",
        "npm test",
        "npm run build",
        "npm run lint",
        "npm run typecheck",
        "pnpm test",
        "pnpm build",
        "bun test",
        "composer test",
        "vue-tsc",
        "npx vue-tsc",
        "godot",
        "godot4",
        "sonar",
        "sonar-scanner",
    ]
    .iter()
    .any(|p| cmd == *p || cmd.starts_with(&format!("{p} ")))
}

pub(super) fn strip_tool_namespace(tool_name: Option<&str>) -> Option<&str> {
    let name = tool_name.map(str::trim).filter(|s| !s.is_empty())?;
    let bare = name.rsplit("__").next().unwrap_or(name);
    Some(bare.rsplit(':').next().unwrap_or(bare))
}

/// Returns `true` when `tool_name` is strictly read-only and safe while Circuit Breaker is OPEN.
pub fn is_read_only_tool(tool_name: Option<&str>) -> bool {
    let Some(bare) = strip_tool_namespace(tool_name) else {
        return false;
    };
    matches!(
        bare,
        "read_file"
            | "Read"
            | "view_file"
            | "view_file_outline"
            | "view_code_item"
            | "grep_search"
            | "find_by_name"
            | "list_dir"
            | "Grep"
            | "Glob"
            | "LS"
            | "list_resources"
            | "read_resource"
            | "mem_search"
            | "mem_timeline"
            | "mem_get"
            | "mem_context"
            | "task_list"
            | "task_get"
            | "code_exec"
            | "context_pack"
    ) || bare.starts_with("graph_")
}

/// Returns `true` when `tool_name` mutates workspace files and warrants Tier 0 + graph + mem checks.
pub fn is_mutating_file_tool(tool_name: Option<&str>) -> bool {
    let Some(bare) = strip_tool_namespace(tool_name) else {
        return false;
    };
    matches!(
        bare,
        "edit_file"
            | "write_file"
            | "write_to_file"
            | "replace_file_content"
            | "multi_replace_file_content"
            | "Edit"
            | "Write"
            | "MultiEdit"
            | "NotebookEdit"
            | "apply_patch"
    )
}

/// Returns `true` when `cmd` safely invokes `anvil circuit status`.
pub fn is_circuit_status_command(cmd: &str) -> bool {
    circuit_subcommand(cmd) == Some("status")
}

/// Returns `true` when `cmd` safely invokes `anvil circuit reset`.
pub fn is_circuit_reset_command(cmd: &str) -> bool {
    circuit_subcommand(cmd) == Some("reset")
}

/// Returns `true` when `cmd` is already wrapped with `anvil run`.
pub fn is_anvil_run_command(cmd: &str) -> bool {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let Some(idx) = skip_anvil_global_flags(&tokens) else {
        return false;
    };
    tokens.get(idx).copied() == Some("run")
}

fn circuit_subcommand(cmd: &str) -> Option<&str> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() || contains_shell_metacharacters(trimmed) {
        return None;
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    let idx = skip_anvil_global_flags(&tokens)?;
    if tokens.get(idx).copied() == Some("circuit") && tokens.len() == idx + 2 {
        return tokens.get(idx + 1).copied();
    }
    None
}

fn skip_anvil_global_flags(tokens: &[&str]) -> Option<usize> {
    let first = tokens.first()?;
    let bin = first.trim_matches(|c| c == '"' || c == '\'');
    let file_name = Path::new(bin)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(bin);
    if !file_name.eq_ignore_ascii_case("anvil") && !file_name.eq_ignore_ascii_case("anvil.exe") {
        return None;
    }
    let mut idx = 1usize;
    while let Some(tok) = tokens.get(idx).copied() {
        if matches!(tok, "--root" | "--repo") {
            idx += 2;
        } else if tok.starts_with("--root=") || tok.starts_with("--repo=") {
            idx += 1;
        } else {
            break;
        }
    }
    Some(idx)
}

/// Extracts and normalizes a repository-relative target file path from `tool_input`.
pub fn extract_rel_file(root: &Path, tool_input: Option<&Value>) -> Option<String> {
    let raw = extract_raw_file_path(tool_input)?;
    let rel = relativize_path(root, Path::new(raw));
    let norm = CodeGraph::normalize_rel_path(&rel);
    (!norm.is_empty()).then_some(norm)
}
