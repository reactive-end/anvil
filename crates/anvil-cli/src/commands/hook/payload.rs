//! Hook stdin/tool-input extraction helpers for `anvil hook`.

use std::path::{Path, PathBuf};

use anvil_core::git;
use anvil_graph::model::CodeGraph;
use serde_json::Value;

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

/// Extracts `(field_key, trimmed_command)` from a `PreToolUse` / `PostToolUse` `tool_input`.
pub fn extract_command_field(tool_input: Option<&Value>) -> Option<(&'static str, &str)> {
    let obj = tool_input?.as_object()?;
    ["CommandLine", "command", "cmd"]
        .into_iter()
        .find_map(|k| obj.get(k).and_then(Value::as_str).map(|s| (k, s.trim())))
}

/// Returns `true` if a shell command should be transparently wrapped with `anvil run --`.
pub fn should_wrap_with_anvil_run(cmd: &str) -> bool {
    if cmd.starts_with("anvil ") || cmd.contains("anvil run") {
        return false;
    }
    [
        "tsc", "npx tsc", "eslint", "npx eslint", "biome", "vitest", "npx vitest", "jest",
        "npx jest", "pytest", "ruff", "dart analyze", "flutter test", "./gradlew", "gradlew",
    ]
    .iter()
    .any(|p| cmd.starts_with(p))
}

/// Extracts and normalizes a repository-relative target file path from `tool_input`.
pub fn extract_rel_file(root: &Path, tool_input: Option<&Value>) -> Option<String> {
    let obj = tool_input?.as_object()?;
    let raw = ["TargetFile", "file_path", "path", "AbsolutePath", "target_file"]
        .iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_str))?;
    let p = Path::new(raw);
    let rel = if p.is_absolute() {
        p.strip_prefix(root)
            .ok()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| raw.to_owned())
    } else {
        raw.to_owned()
    };
    let norm = CodeGraph::normalize_rel_path(&rel);
    (!norm.is_empty()).then_some(norm)
}
