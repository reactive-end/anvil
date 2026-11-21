//! Hook stdin/tool-input extraction and tool classification helpers for `anvil hook`.

use std::path::{Path, PathBuf};

use anvil_core::fs_util::relativize_path;
use anvil_core::git;
use anvil_graph::model::CodeGraph;
use serde_json::Value;

const SHELL_METACHARS: &[&str] = &["&&", "||", "|", ";", ">", "<", "$(", "`", "\n", "\r"];

/// Resolves the effective repository root from a hook's `cwd` field.
pub fn resolve_hook_root(default_root: &Path, cwd: Option<&str>) -> PathBuf {
    let Some(raw) = cwd.map(str::trim).filter(|s| !s.is_empty()) else {
        return default_root.to_path_buf();
    };
    let path = PathBuf::from(raw);
    if path.is_dir() {
        git::repo_root(&path).unwrap_or(path)
    } else {
        default_root.to_path_buf()
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
    ]
    .iter()
    .any(|p| cmd == *p || cmd.starts_with(&format!("{p} ")))
}

fn strip_tool_namespace(tool_name: Option<&str>) -> Option<&str> {
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
            | "replace_file_content"
            | "multi_replace_file_content"
            | "Edit"
            | "Write"
            | "MultiEdit"
            | "NotebookEdit"
            | "apply_patch"
    )
}

/// Returns `true` when `cmd` safely invokes `anvil circuit reset` or `anvil circuit status`.
pub fn is_circuit_recovery_command(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    if trimmed.is_empty() || contains_shell_metacharacters(trimmed) {
        return false;
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    let Some(first) = tokens.first() else {
        return false;
    };
    let bin = first.trim_matches(|c| c == '"' || c == '\'');
    let file_name = Path::new(bin)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(bin);
    if !file_name.eq_ignore_ascii_case("anvil") && !file_name.eq_ignore_ascii_case("anvil.exe") {
        return false;
    }

    let mut idx = 1usize;
    if tokens.get(idx).copied() == Some("--root") {
        idx += 2;
    } else if tokens.get(idx).is_some_and(|t| t.starts_with("--root=")) {
        idx += 1;
    }
    tokens.get(idx).copied() == Some("circuit")
        && matches!(tokens.get(idx + 1).copied(), Some("reset" | "status"))
        && tokens.len() == idx + 2
}

/// Extracts and normalizes a repository-relative target file path from `tool_input`.
pub fn extract_rel_file(root: &Path, tool_input: Option<&Value>) -> Option<String> {
    let obj = tool_input?.as_object()?;
    let raw = ["TargetFile", "file_path", "path", "AbsolutePath", "target_file"]
        .iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_str))?;
    let rel = relativize_path(root, Path::new(raw));
    let norm = CodeGraph::normalize_rel_path(&rel);
    (!norm.is_empty()).then_some(norm)
}
