//! Role enforcement for orchestrator mode: the orchestrator judges, executors write within scope.

use std::path::{Path, PathBuf};

use anvil_adapters::claude_code::agents::EXECUTOR_AGENT;
use anvil_adapters::contract::{HookOutcome, InboxMessage, MessageLevel, NormalizedHookInput};
use anvil_core::orchestrator_mode::{
    self, grant_scope, granted_scope, overlapping_entry, parse_scope, recent_spawns, record_spawn,
    scope_allows,
};
use serde_json::Value;

use super::executors::{self, brief_scope, subagents_dir};
use super::payload::{
    extract_command_field, extract_rel_file, extract_target_file_path, is_mutating_file_tool,
    is_within_root, resolve_hook_root, strip_tool_namespace,
};
use super::roles_shell;
use super::shell::targets::is_scratch_path;

const EXECUTOR_FORBIDDEN_TOOLS: &[&str] =
    &["Agent", "Task", "Workflow", "task_start", "task_done", "task_skip", "task_record"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// The main thread: plans, delegates, and reviews.
    Orchestrator,
    /// `anvil-executor`: the only role that writes code.
    Executor,
    /// Any other subagent: read-only help for the orchestrator.
    Researcher,
}

/// Applies role rules to a tool call, or returns `None` when the mode is off or the call is fine.
pub fn check_roles(root: &Path, input: &NormalizedHookInput) -> Option<HookOutcome> {
    let home = enabled_root(root, input)?;
    match role_of(input) {
        Role::Executor => check_executor(&home, input),
        Role::Orchestrator => {
            check_delegation(&home, input).or_else(|| check_read_only(&home, input))
        }
        Role::Researcher => check_read_only(&home, input),
    }
}

/// Returns the standing instructions injected while the mode is on.
pub fn mode_reminder(root: &Path) -> Option<InboxMessage> {
    orchestrator_mode::is_enabled(root).then(|| {
        InboxMessage::new(
            "orchestrator-mode",
            MessageLevel::Info,
            format!(
                "ON. You are the orchestrator: understand, plan, decide, and review, but never write code. Delegate every edit to the `{EXECUTOR_AGENT}` agent with a self-contained brief that contains a `SCOPE: path, path` line (exact files or directories), the constraints, and the done command. Review with `anvil diff` and `anvil check`, and send corrections back to the executor. Only the user can turn this mode off."
            ),
            None,
        )
    })
}

fn enabled_root(root: &Path, input: &NormalizedHookInput) -> Option<PathBuf> {
    let session_root = resolve_hook_root(root, input.cwd.as_deref());
    [session_root, root.to_path_buf()]
        .into_iter()
        .find(|candidate| orchestrator_mode::is_enabled(candidate))
}

fn role_of(input: &NormalizedHookInput) -> Role {
    if input.agent_id.is_none() {
        return Role::Orchestrator;
    }
    let kind = input.agent_type.as_deref().unwrap_or_default();
    if kind.rsplit(':').next() == Some(EXECUTOR_AGENT) { Role::Executor } else { Role::Researcher }
}

/// Requires a bounded scope on every brief and keeps the configured executor model and effort.
fn check_delegation(root: &Path, input: &NormalizedHookInput) -> Option<HookOutcome> {
    let tool = strip_tool_namespace(input.tool_name.as_deref())?;
    let args = input.tool_input.as_ref()?.as_object()?;
    let session = input.session_id.as_deref().unwrap_or("default");
    let text = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or_default();
    let dir = subagents_dir(input.transcript_path.as_deref(), input.session_id.as_deref());
    if tool == "SendMessage" {
        let Ok(paths) = parse_scope(text("message")) else {
            return None;
        };
        if let Some(clash) = dir
            .as_deref()
            .and_then(|d| find_clash(root, d, session, &paths, Some(text("to")), None))
        {
            return Some(clash);
        }
        let _ = grant_scope(root, session, &paths);
        return None;
    }
    let is_executor = text("subagent_type").rsplit(':').next() == Some(EXECUTOR_AGENT);
    if !matches!(tool, "Agent" | "Task") || !is_executor {
        return None;
    }
    let paths = match parse_scope(text("prompt")) {
        Ok(paths) => paths,
        Err(err) => {
            return Some(HookOutcome::deny(format!(
                "Orchestrator mode: the executor brief has no usable scope ({err}). Add a line such as `SCOPE: src/auth.rs, tests/auth.rs` naming what it may edit."
            )));
        }
    };
    let call = input.tool_use_id.as_deref();
    if let Some(clash) =
        dir.as_deref().and_then(|d| find_clash(root, d, session, &paths, None, call))
    {
        return Some(clash);
    }
    let recorded = grant_scope(root, session, &paths)
        .and_then(|()| call.map_or(Ok(()), |id| record_spawn(root, session, id, &paths)));
    if let Err(err) = recorded {
        return Some(HookOutcome::deny(format!(
            "Orchestrator mode: could not record the executor scope ({err})."
        )));
    }
    if !args.contains_key("model") && !args.contains_key("effort") {
        return None;
    }
    let mut kept = args.clone();
    kept.remove("model");
    kept.remove("effort");
    Some(HookOutcome::overwrite(Value::Object(kept), None))
}

fn check_read_only(root: &Path, input: &NormalizedHookInput) -> Option<HookOutcome> {
    if is_mutating_file_tool(input.tool_name.as_deref()) {
        let target = extract_target_file_path(input.tool_input.as_ref(), root);
        if target.is_some_and(|abs| !is_within_root(root, &abs) && is_private_note(&abs)) {
            return None;
        }
        return Some(HookOutcome::deny(format!(
            "Orchestrator mode: only `{EXECUTOR_AGENT}` edits files. Delegate this change with the Agent tool and a `SCOPE:` line."
        )));
    }
    let (_, cmd) = extract_command_field(input.tool_input.as_ref())?;
    let reason = roles_shell::why_not_read_only(cmd)?;
    Some(HookOutcome::deny(format!(
        "Orchestrator mode: {reason}. This role only inspects and verifies; delegate the rest to `{EXECUTOR_AGENT}`."
    )))
}

/// Plans, memories, and scratch files outside the repository are the orchestrator's own notes.
fn is_private_note(path: &Path) -> bool {
    let norm = path.to_string_lossy().replace('\\', "/");
    norm.contains("/.claude/") || is_scratch_path(&norm)
}

fn check_executor(root: &Path, input: &NormalizedHookInput) -> Option<HookOutcome> {
    let tool = strip_tool_namespace(input.tool_name.as_deref())?;
    if EXECUTOR_FORBIDDEN_TOOLS.contains(&tool) {
        return Some(HookOutcome::deny(format!(
            "Orchestrator mode: `{tool}` belongs to the orchestrator. Finish your step and report back."
        )));
    }
    let scope = executor_scope(root, input);
    if is_mutating_file_tool(Some(tool)) {
        let abs = extract_target_file_path(input.tool_input.as_ref(), root)?;
        if !is_within_root(root, &abs) && is_scratch_path(&abs.to_string_lossy()) {
            return None;
        }
        let rel = extract_rel_file(root, input.tool_input.as_ref()).unwrap_or_default();
        return (!is_within_root(root, &abs) || !scope_allows(&scope, &rel))
            .then(|| out_of_scope(&rel, &scope));
    }
    let (_, cmd) = extract_command_field(input.tool_input.as_ref())?;
    if let Some(reason) = roles_shell::why_not_executor(cmd) {
        return Some(HookOutcome::deny(format!("Orchestrator mode: {reason}.")));
    }
    let cwd = input.cwd.as_deref().map_or_else(|| root.to_path_buf(), PathBuf::from);
    roles_shell::written_paths(cmd).into_iter().find_map(|raw| {
        let abs = cwd.join(&raw);
        if !is_within_root(root, &abs) {
            return (!is_scratch_path(&abs.to_string_lossy())).then(|| out_of_scope(&raw, &scope));
        }
        let path_input = serde_json::json!({ "file_path": abs.to_string_lossy() });
        let rel = extract_rel_file(root, Some(&path_input)).unwrap_or_default();
        let is_build_output = rel
            .split('/')
            .any(|seg| matches!(seg, "target" | "dist" | "build" | "node_modules" | "coverage"));
        (!is_build_output && !scope_allows(&scope, &rel)).then(|| out_of_scope(&rel, &scope))
    })
}

/// The executor's own latest brief; the session's delegated paths when its transcript is unreadable.
fn executor_scope(root: &Path, input: &NormalizedHookInput) -> Vec<String> {
    let dir = subagents_dir(input.transcript_path.as_deref(), input.session_id.as_deref());
    dir.zip(input.agent_id.as_deref())
        .and_then(|(dir, agent)| brief_scope(&dir, agent))
        .unwrap_or_else(|| granted_scope(root, input.session_id.as_deref().unwrap_or("default")))
}

/// Refuses a brief whose scope may touch a file a running or just-started executor can edit.
/// `target` is the executor being briefed again, `call` the `Agent` call starting a new one.
fn find_clash(
    root: &Path,
    dir: &Path,
    session: &str,
    paths: &[String],
    target: Option<&str>,
    call: Option<&str>,
) -> Option<HookOutcome> {
    let known = executors::executors(dir);
    let running = known
        .iter()
        .filter(|e| {
            e.running && !target.is_some_and(|t| t == e.agent_id || e.name.as_deref() == Some(t))
        })
        .filter_map(|e| {
            Some((format!("executor `{}`", e.agent_id), brief_scope(dir, &e.agent_id)?))
        });
    let starting = recent_spawns(root, session).into_iter().filter(|(id, _)| {
        Some(id.as_str()) != call && !known.iter().any(|e| e.tool_use_id.as_deref() == Some(id))
    });
    let starting =
        starting.map(|(id, scope)| (format!("the executor being started by `{id}`"), scope));
    running.chain(starting).find_map(|(who, scope)| {
        let (mine, theirs) = overlapping_entry(paths, &scope)?;
        Some(HookOutcome::deny(format!(
            "Orchestrator mode: `{mine}` in this brief overlaps `{theirs}`, which {who} may still edit. Executors that run at the same time must not share files: give the shared file to one of them, or wait for it to finish."
        )))
    })
}

fn out_of_scope(rel: &str, scope: &[String]) -> HookOutcome {
    let allowed = if scope.is_empty() { "nothing".to_owned() } else { scope.join(", ") };
    HookOutcome::deny(format!(
        "Orchestrator mode: `{rel}` is outside the delegated scope ({allowed}). Do not work around this; report to the orchestrator what you need and why."
    ))
}
