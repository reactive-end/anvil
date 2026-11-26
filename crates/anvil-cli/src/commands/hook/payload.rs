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
    "notebook_path",
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
        && let Some(start_dir) = find_existing_ancestor(&target_file)
        && let Some(file_root) = find_initialized_root_upward(start_dir)
        && !paths_equal(&file_root, &cwd_root)
    {
        return (file_root, Some(cwd_root));
    }
    (cwd_root, None)
}

fn find_existing_ancestor(path: &Path) -> Option<&Path> {
    path.ancestors().find(|p| p.is_dir())
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
    find_initialized_root_upward(&path).or_else(|| git::repo_root(&path).ok()).unwrap_or(path)
}

fn find_initialized_root_upward(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|dir| dir.join(".harness").is_dir()).map(Path::to_path_buf)
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

pub(super) fn extract_target_file_path(
    tool_input: Option<&Value>,
    cwd_root: &Path,
) -> Option<PathBuf> {
    let raw = extract_raw_file_path(tool_input)?;
    let p = PathBuf::from(raw);
    if p.is_absolute() { Some(p) } else { Some(cwd_root.join(p)) }
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
    let trimmed = cmd.trim();
    if trimmed.is_empty() || contains_shell_metacharacters(trimmed) || trimmed.contains("anvil run")
    {
        return false;
    }
    let tokens = split_shell_tokens(trimmed);
    let Some(first) = tokens.first() else {
        return false;
    };
    let prog = normalize_bin_name(first);
    if prog == "anvil" {
        return false;
    }
    let rest: Vec<String> = tokens[1..].iter().map(|t| normalize_bin_name(t)).collect();
    matches_wrappable_command(&prog, &rest, &tokens[1..])
}

fn matches_wrappable_command(prog: &str, norm_rest: &[String], raw_rest: &[String]) -> bool {
    if matches!(
        prog,
        "tsc"
            | "tsgo"
            | "eslint"
            | "biome"
            | "vitest"
            | "jest"
            | "pytest"
            | "ruff"
            | "pyright"
            | "mypy"
            | "ktlint"
            | "detekt"
            | "gdlint"
            | "gradlew"
            | "phpstan"
            | "psalm"
            | "phpunit"
            | "pest"
            | "pint"
            | "vue-tsc"
            | "godot"
            | "godot4"
            | "sonar"
            | "sonar-scanner"
    ) {
        return true;
    }
    if matches!(prog, "npx" | "bunx")
        && let Some(wrapped) = raw_rest.iter().find(|t| !t.starts_with('-'))
    {
        return matches!(
            normalize_bin_name(wrapped).as_str(),
            "tsc" | "tsgo" | "eslint" | "biome" | "vitest" | "jest" | "vue-tsc" | "pyright"
        );
    }
    let two = (prog, norm_rest.first().map(String::as_str).unwrap_or(""));
    if matches!(
        two,
        ("cargo", "test" | "check" | "clippy" | "build")
            | ("npm" | "pnpm" | "yarn", "test" | "build" | "lint")
            | ("bun" | "composer", "test")
            | ("dart", "analyze" | "test")
            | ("flutter", "test" | "analyze")
            | ("node", "--test")
    ) {
        return true;
    }
    let raw_two = raw_rest.get(1).map(String::as_str).unwrap_or("");
    matches!(
        (prog, raw_rest.first().map(String::as_str).unwrap_or(""), raw_two),
        ("npm", "run", "build" | "lint" | "typecheck")
            | ("php", "artisan", "test")
            | ("python" | "python3" | "py", "-m", "pytest")
    )
}

fn normalize_bin_name(token: &str) -> String {
    let base = token.rsplit(['/', '\\']).next().unwrap_or(token);
    let lower = base.to_ascii_lowercase();
    lower
        .strip_suffix(".exe")
        .or_else(|| lower.strip_suffix(".cmd"))
        .or_else(|| lower.strip_suffix(".bat"))
        .unwrap_or(&lower)
        .to_owned()
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
            | "task_resume"
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
    circuit_subcommand(cmd).as_deref() == Some("status")
}

/// Returns `true` when `cmd` safely invokes `anvil circuit reset`.
pub fn is_circuit_reset_command(cmd: &str) -> bool {
    circuit_subcommand(cmd).as_deref() == Some("reset")
}

/// Returns `true` when `cmd` is already wrapped with `anvil run`.
pub fn is_anvil_run_command(cmd: &str) -> bool {
    let tokens = split_shell_tokens(cmd);
    let Some(idx) = skip_anvil_global_flags(&tokens) else {
        return false;
    };
    tokens.get(idx).map(String::as_str) == Some("run")
}

fn circuit_subcommand(cmd: &str) -> Option<String> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() || contains_shell_metacharacters(trimmed) {
        return None;
    }
    let tokens = split_shell_tokens(trimmed);
    let idx = skip_anvil_global_flags(&tokens)?;
    if tokens.get(idx).map(String::as_str) == Some("circuit") && tokens.len() == idx + 2 {
        return tokens.get(idx + 1).cloned();
    }
    None
}

fn skip_anvil_global_flags(tokens: &[String]) -> Option<usize> {
    let first = tokens.first()?;
    if normalize_bin_name(first) != "anvil" {
        return None;
    }
    let mut idx = 1usize;
    while let Some(tok) = tokens.get(idx).map(String::as_str) {
        if matches!(tok, "--root" | "--repo") {
            idx += 2;
        } else if tok == "--no-color" || tok.starts_with("--root=") || tok.starts_with("--repo=") {
            idx += 1;
        } else {
            break;
        }
    }
    Some(idx)
}

fn split_shell_tokens(cmd: &str) -> Vec<String> {
    let (mut out, mut cur, mut quote) = (Vec::new(), String::new(), None::<char>);
    let mut chars = cmd.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(q) = quote {
            if ch == '\\' && q == '"' && chars.peek().is_some_and(|n| matches!(*n, '"' | '\\')) {
                if let Some(next_ch) = chars.next() {
                    cur.push(next_ch);
                }
            } else if ch == q {
                quote = None;
            } else {
                cur.push(ch);
            }
        } else if matches!(ch, '"' | '\'') {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Extracts and normalizes a repository-relative target file path from `tool_input`.
pub fn extract_rel_file(root: &Path, tool_input: Option<&Value>) -> Option<String> {
    let raw = extract_raw_file_path(tool_input)?;
    let path = Path::new(raw);
    let rel = if path.is_absolute() {
        let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let target_canon = canonicalize_with_existing_ancestor(path);
        relativize_path(&root_canon, &target_canon)
    } else {
        relativize_path(root, path)
    };
    let norm = CodeGraph::normalize_rel_path(&rel);
    (!norm.is_empty()).then_some(norm)
}

/// Returns `true` when `target` is inside `root` (handles non-existent new paths too).
pub fn is_within_root(root: &Path, target: &Path) -> bool {
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let target_canon = canonicalize_with_existing_ancestor(target);
    let rel = relativize_path(&root_canon, &target_canon);
    let is_abs_or_parent = rel.starts_with('/')
        || rel.starts_with("../")
        || rel == ".."
        || (rel.len() >= 2 && rel.as_bytes()[1] == b':');
    !is_abs_or_parent
}

fn canonicalize_with_existing_ancestor(path: &Path) -> PathBuf {
    if let Ok(c) = path.canonicalize() {
        return c;
    }
    let mut missing = Vec::new();
    let mut cur = Some(path);
    while let Some(p) = cur {
        if p.exists() {
            let mut base = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
            for seg in missing.into_iter().rev() {
                base.push(seg);
            }
            return base;
        }
        if let Some(name) = p.file_name() {
            missing.push(name.to_os_string());
        }
        cur = p.parent();
    }
    path.to_path_buf()
}
