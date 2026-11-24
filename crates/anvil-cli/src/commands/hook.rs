//! `anvil hook`: unified Antigravity & Claude Code hook dispatcher.

mod error_log;
mod gates;
pub mod git_install;
mod payload;
mod security_guard;
mod task_auto;
mod token_guard;

use std::io::{self, Read};
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use anvil_adapters::contract::{
    AdapterKind, HookEvent, HookOutcome, InboxMessage, MessageLevel, NormalizedHookInput,
};
use anvil_adapters::{antigravity, claude_code};
use anvil_core::circuit_breaker::CircuitBreaker;
use anvil_core::config::AnvilConfig;
use anvil_core::diff_classifier::{ChangedFile, classify_files};
use anvil_core::fs_util::with_file_lock;
use anvil_core::git::{self, DiffScope};
use anvil_core::stats::{StatsEvent, record_event};
use anvil_graph::CodeGraphEngine;
use anvil_graph::builder::update_file;
use anvil_graph::store;
use anvil_memory::MemoryEngine;
use anvil_tasks::resolve_author_fast;
use serde_json::Value;

use crate::daemon::protocol::{DaemonRequest, DaemonResponse};
use crate::daemon::try_request;
use gates::{collect_changed_gate_errors, collect_post_tool_messages, update_circuit_from_output};
use payload::{
    extract_command_field, extract_rel_file, is_circuit_reset_command, is_circuit_status_command,
    is_mutating_file_tool, is_read_only_tool, resolve_hook_targets, should_bypass_stop,
    should_wrap_with_anvil_run,
};

/// Executes `anvil hook <event> --adapter <antigravity|claude>`.
pub fn execute(root: &Path, event_raw: &str, adapter_raw: &str) -> Result<()> {
    if event_raw.eq_ignore_ascii_case("install-git") {
        let updated = git_install::install_pre_commit_hook(root)?;
        println!(
            "{}",
            if updated {
                "installed `.git/hooks/pre-commit`"
            } else {
                "`.git/hooks/pre-commit` unchanged or no `.git` directory"
            }
        );
        return Ok(());
    }
    let started = Instant::now();
    let adapter = AdapterKind::parse(adapter_raw)?;
    let event = HookEvent::parse(event_raw)?;
    let mut raw_stdin = String::new();
    io::stdin().read_to_string(&mut raw_stdin)?;

    let input = match adapter {
        AdapterKind::Antigravity => antigravity::parse_input(&raw_stdin, Some(event))?,
        AdapterKind::ClaudeCode => claude_code::parse_input(&raw_stdin, Some(event))?,
    };
    let (effective_root, _) =
        resolve_hook_targets(root, input.cwd.as_deref(), input.tool_input.as_ref());
    let config = AnvilConfig::load_or_default(&effective_root)?;

    let outcome = match event {
        HookEvent::SessionStart => handle_session_start(&effective_root, &config),
        HookEvent::UserPromptSubmit | HookEvent::PreInvocation => {
            if effective_root.join(".harness").is_dir() {
                task_auto::handle_prompt_hook(&effective_root, &input)
            } else {
                HookOutcome::allow()
            }
        }
        HookEvent::PreToolUse => handle_pre_tool_use(&effective_root, &config, &input),
        HookEvent::PostToolUse => handle_post_tool_use(&effective_root, &config, &input),
        HookEvent::Stop => handle_stop(&effective_root, &config, should_bypass_stop(&raw_stdin)),
    };

    let rendered = match adapter {
        AdapterKind::Antigravity => antigravity::render_output(&effective_root, event, &outcome)?,
        AdapterKind::ClaudeCode => claude_code::render_output(&effective_root, event, &outcome)?,
    };
    println!("{}", serde_json::to_string(&rendered)?);

    if effective_root.join(".harness").is_dir() {
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let _ = record_event(&effective_root, StatsEvent::HookCall { elapsed_ms });
    }
    Ok(())
}

fn handle_session_start(root: &Path, config: &AnvilConfig) -> HookOutcome {
    if !root.join(".harness").is_dir() {
        return HookOutcome::allow();
    }
    crate::commands::daemon::ensure_daemon_running_async(root);
    let mut messages = Vec::new();
    let author = resolve_author_fast(root).unwrap_or_else(|_| "hook".to_owned());
    append_session_memory(root, &author, &mut messages);
    append_session_graph(root, &mut messages);
    append_session_task_context(root, &mut messages);
    match CircuitBreaker::new(root, config.circuit_breaker_limit).status() {
        Ok(st) if st.is_open() => {
            let msg = "Circuit Breaker is OPEN; run `anvil circuit reset` after fixing.".to_owned();
            messages.push(InboxMessage::new(
                "circuit-breaker",
                MessageLevel::Error,
                msg,
                None,
            ));
        }
        Ok(_) => {}
        Err(err) => error_log::log_hook_error(root, "session-start:circuit", &err),
    }
    HookOutcome::with_messages(messages)
}

fn append_session_memory(root: &Path, author: &str, messages: &mut Vec<InboxMessage>) {
    match MemoryEngine::open(root, author) {
        Ok(mem) => match mem.session_context(450) {
            Ok(hits) if !hits.is_empty() => {
                let summary: Vec<String> = hits
                    .into_iter()
                    .map(|h| format!("[{}] {}", h.kind, h.title))
                    .collect();
                messages.push(InboxMessage::new(
                    "anvil-memory",
                    MessageLevel::Info,
                    summary.join("; "),
                    None,
                ));
            }
            Ok(_) => {}
            Err(err) => error_log::log_hook_error(root, "session-start:mem-context", &err),
        },
        Err(err) => error_log::log_hook_error(root, "session-start:mem-open", &err),
    }
}

fn append_session_graph(root: &Path, messages: &mut Vec<InboxMessage>) {
    let bin_path = store::default_bin_path(root);
    if !bin_path.is_file() {
        return;
    }
    match store::read_header(&bin_path) {
        Ok(header) => {
            let msg = format!(
                "Graph ready ({} nodes, {} edges)",
                header.live_nodes, header.live_edges
            );
            messages.push(InboxMessage::new("anvil-graph", MessageLevel::Info, msg, None));
        }
        Err(err) => error_log::log_hook_error(root, "session-start:graph-header", &err),
    }
}

fn append_session_task_context(root: &Path, messages: &mut Vec<InboxMessage>) {
    match anvil_tasks::build_resume_brief(root, None) {
        Ok(Some(brief)) => {
            messages.push(InboxMessage::new(
                "anvil-tasks",
                MessageLevel::Warning,
                format_resume_inbox_msg(&brief),
                None,
            ));
            return;
        }
        Ok(None) => {}
        Err(err) => error_log::log_hook_error(root, "session-start:resume-brief", &err),
    }
    match anvil_tasks::consolidate_repo_tasks(root) {
        Ok(consolidated) => {
            let queued: Vec<_> = consolidated
                .tasks
                .iter()
                .filter(|t| t.status == anvil_tasks::TaskStatus::NotStarted)
                .collect();
            if let Some(next) = queued.first() {
                let msg = format!(
                    "Queued tasks: {} not_started (next: {} {}). Work STRICTLY one task at a time (complete start -> code -> checkpoint update -> verify -> done on one task before starting the next).",
                    queued.len(),
                    next.task_id,
                    next.request
                );
                messages.push(InboxMessage::new("anvil-tasks", MessageLevel::Info, msg, None));
            } else if let Some(latest) = consolidated.tasks.last() {
                let msg = format!("Latest task {}: {}", latest.task_id, latest.request);
                messages.push(InboxMessage::new("anvil-tasks", MessageLevel::Info, msg, None));
            }
        }
        Err(err) => error_log::log_hook_error(root, "session-start:tasks", &err),
    }
}

fn format_resume_inbox_msg(brief: &anvil_tasks::TaskResumeBrief) -> String {
    let mut parts = vec![format!(
        "RESUME interrupted task {} [{}] \"{}\" ({})",
        brief.task_id, brief.status, brief.title, brief.progress_file
    )];
    if !brief.summary.is_empty() {
        parts.push(format!("Last: {}", brief.summary));
    }
    if !brief.next_steps.is_empty() {
        parts.push(format!("Next: {}", brief.next_steps.join("; ")));
    }
    if !brief.touched_files.is_empty() {
        parts.push(format!("Touched: {}", brief.touched_files.join(", ")));
    }
    if let Some(cmd) = brief.last_command.as_deref() {
        let status = match brief.last_command_passed {
            Some(true) => "PASS",
            Some(false) => "FAIL",
            None => "UNKNOWN",
        };
        parts.push(format!("LastCmd: [{status}] {cmd}"));
    }
    parts.push("Run `task_resume` / `anvil tasks resume` before restarting from scratch.".to_owned());
    parts.join(" | ")
}

fn run_pre_tool_guards(root: &Path, input: &NormalizedHookInput) -> Option<HookOutcome> {
    security_guard::check_security_guard(root, input)
        .or_else(|| token_guard::check_read_token_guard(root, input))
}

fn handle_pre_tool_use(
    root: &Path,
    config: &AnvilConfig,
    input: &NormalizedHookInput,
) -> HookOutcome {
    let cmd_field = extract_command_field(input.tool_input.as_ref());
    if root.join(".harness").is_dir() {
        match CircuitBreaker::new(root, config.circuit_breaker_limit).status() {
            Ok(st) if st.is_open() => {
                let is_status = cmd_field.is_some_and(|(_, cmd)| is_circuit_status_command(cmd));
                let is_reset = cmd_field.is_some_and(|(_, cmd)| is_circuit_reset_command(cmd));
                if is_read_only_tool(input.tool_name.as_deref()) || is_status {
                    return run_pre_tool_guards(root, input).unwrap_or_else(HookOutcome::allow);
                }
                if is_reset {
                    return HookOutcome::ask(format!(
                        "Circuit Breaker is OPEN ({} identical failures) — confirm before running `anvil circuit reset`.",
                        st.consecutive_failures
                    ));
                }
                let tool = input.tool_name.as_deref().unwrap_or("tool");
                return HookOutcome::deny(format!(
                    "Circuit Breaker OPEN ({} identical failures) — blocked `{tool}`. Run `anvil circuit reset` after fixing.",
                    st.consecutive_failures
                ));
            }
            Ok(_) => {}
            Err(err) => error_log::log_hook_error(root, "pre-tool-use:circuit", &err),
        }
    }
    if let Some(guard_outcome) = run_pre_tool_guards(root, input) {
        return guard_outcome;
    }
    if let Some((key, cmd)) = cmd_field {
        if cmd.contains("--no-verify") {
            return HookOutcome::deny("Forbidden `--no-verify` bypass flag detected.");
        }
        if should_wrap_with_anvil_run(cmd)
            && let Some(mut obj) = input.tool_input.as_ref().and_then(Value::as_object).cloned()
        {
            obj.insert(key.to_owned(), Value::String(format!("anvil run -- {cmd}")));
            return HookOutcome::overwrite(
                Value::Object(obj),
                Some("Wrapped command with `anvil run`".to_owned()),
            );
        }
    }
    HookOutcome::allow()
}

fn handle_post_tool_use(
    root: &Path,
    config: &AnvilConfig,
    input: &NormalizedHookInput,
) -> HookOutcome {
    let mut messages = Vec::new();
    if is_mutating_file_tool(input.tool_name.as_deref())
        && let Some(rel) = extract_rel_file(root, input.tool_input.as_ref())
    {
        task_auto::mark_turn_edit(root);
        collect_post_tool_messages(root, &rel, config, &mut messages);
        update_graph_after_edit(root, &rel, config);
        match MemoryEngine::open_without_sync(root, "hook") {
            Ok(mut mem) => {
                if let Err(err) = mem.mark_files_changed(std::slice::from_ref(&rel)) {
                    error_log::log_hook_error(root, "post-tool-use:mem-mark-changed", &err);
                }
            }
            Err(err) => error_log::log_hook_error(root, "post-tool-use:mem-open", &err),
        }
        match anvil_tasks::record_session_edit(root, &rel) {
            Ok(true) => {
                messages.push(InboxMessage::new(
                    "anvil-tasks",
                    MessageLevel::Info,
                    "Edits tracked with no active in_progress task. Work strictly 1 task at a time: call `task_start`/`anvil tasks start <ID|title>` first, checkpoint with `task_update` after code edits (before tests), and close with `task_done` — or `task_skip` for trivial tweaks.".to_owned(),
                    None,
                ));
            }
            Ok(false) => {}
            Err(err) => error_log::log_hook_error(root, "post-tool-use:task-session-edit", &err),
        }
    }
    update_circuit_from_output(root, config, input, &mut messages);
    HookOutcome::with_messages(messages)
}

fn update_graph_after_edit(root: &Path, rel: &str, config: &AnvilConfig) {
    let updated_via_daemon = matches!(
        try_request(root, DaemonRequest::UpdateFile { rel_path: rel.to_owned() }),
        Some(DaemonResponse::Updated { .. })
    );
    if updated_via_daemon {
        return;
    }
    let bin_path = store::default_bin_path(root);
    if !bin_path.is_file() {
        return;
    }
    let lock_path = root.join(".harness/.state/graph.lock");
    if let Err(err) = with_file_lock(&lock_path, 2000, || {
        let mut graph = store::load_binary(&bin_path)?;
        if update_file(&mut graph, root, rel, config)? {
            store::save_binary(&graph, &bin_path)?;
        }
        Ok(())
    }) {
        error_log::log_hook_error(root, "post-tool-use:graph-update", &err);
    }
}

fn handle_stop(root: &Path, config: &AnvilConfig, bypass_stop: bool) -> HookOutcome {
    if bypass_stop {
        return HookOutcome::allow();
    }
    let has_root_harness = root.join(".harness").is_dir();
    if has_root_harness {
        task_auto::refresh_tasks_json_if_changed(root);
        match CircuitBreaker::new(root, config.circuit_breaker_limit).status() {
            Ok(st) if st.is_open() => {
                let msg = "Circuit Breaker OPEN — stopping for human escalation.".to_owned();
                return HookOutcome::with_messages(vec![InboxMessage::new(
                    "circuit-breaker",
                    MessageLevel::Error,
                    msg,
                    None,
                )]);
            }
            Ok(_) => {}
            Err(err) => error_log::log_hook_error(root, "stop:circuit", &err),
        }
    }
    if let Some(sub_block) = check_subrepo_stop_gates(root, config) {
        return sub_block;
    }
    if !has_root_harness {
        return HookOutcome::allow();
    }
    let changed = git::changed_files(root, DiffScope::WorkingTree).unwrap_or_default();
    if changed.is_empty() || task_auto::should_skip_readonly_stop(root, &changed) {
        return HookOutcome::allow();
    }
    let errs = collect_changed_gate_errors(root, &changed, config);
    if !errs.is_empty() {
        return HookOutcome::block_stop(format!(
            "Tier 0 AST gate failed ({} error(s)): {}",
            errs.len(),
            errs.join("; ")
        ));
    }
    if let Some(block) = task_auto::check_pending_task_summary(root, &changed) {
        return block;
    }
    let mut messages = Vec::new();
    append_stop_advisories(root, config, &changed, &mut messages);
    task_auto::mark_stop_completed(root);
    HookOutcome::with_messages(messages)
}

fn append_stop_advisories(
    root: &Path,
    config: &AnvilConfig,
    changed: &[String],
    messages: &mut Vec<InboxMessage>,
) {
    if let Some(blast) = resolve_stop_blast(root, config, changed)
        && !blast.impacted_files.is_empty()
    {
        let msg = format!(
            "{} impacted file(s), {} related test(s)",
            blast.impacted_files.len(),
            blast.related_tests.len()
        );
        messages.push(InboxMessage::new("tier1:impact", MessageLevel::Info, msg, None));
    }
    let classified = classify_files(&changed.iter().map(ChangedFile::from_path).collect::<Vec<_>>());
    if classified.needs_visual_qa {
        messages.push(InboxMessage::new(
            "tier2:qa",
            MessageLevel::Warning,
            "UI files changed; verify with `anvil qa`.".to_owned(),
            None,
        ));
    }
}

fn resolve_stop_blast(
    root: &Path,
    config: &AnvilConfig,
    changed: &[String],
) -> Option<anvil_graph::impact::BlastRadiusReport> {
    if let Some(DaemonResponse::BlastResult { report }) = try_request(
        root,
        DaemonRequest::BlastRadius {
            files: changed.to_vec(),
            depth: 2,
        },
    ) {
        return Some(report);
    }
    match CodeGraphEngine::load_cached(root, config.clone()) {
        Ok(engine) => Some(engine.blast_radius(changed, 2)),
        Err(err) => {
            error_log::log_hook_error(root, "stop:graph-load", &err);
            None
        }
    }
}

fn check_subrepo_stop_gates(root: &Path, config: &AnvilConfig) -> Option<HookOutcome> {
    let sub_paths = collect_subrepo_stop_paths(root, config);
    if sub_paths.is_empty() {
        return None;
    }
    let root_has_git = root.join(".git").exists();
    for rel_sub in sub_paths {
        let sub_dir = root.join(&rel_sub);
        if !sub_dir.join(".harness").is_dir() || (root_has_git && !sub_dir.join(".git").exists()) {
            continue;
        }
        task_auto::refresh_tasks_json_if_changed(&sub_dir);
        let sub_cfg = AnvilConfig::load_or_default(&sub_dir).unwrap_or_else(|_| config.clone());
        let sub_changed = git::changed_files(&sub_dir, DiffScope::WorkingTree).unwrap_or_default();
        if sub_changed.is_empty() {
            continue;
        }
        let errs = collect_changed_gate_errors(&sub_dir, &sub_changed, &sub_cfg);
        if !errs.is_empty() {
            return Some(HookOutcome::block_stop(format!(
                "Tier 0 AST gate failed in `{rel_sub}` ({} error(s)): {}",
                errs.len(),
                errs.join("; ")
            )));
        }
        if let Some(block) = task_auto::check_pending_task_summary(&sub_dir, &sub_changed) {
            return Some(block);
        }
    }
    None
}

fn collect_subrepo_stop_paths(root: &Path, config: &AnvilConfig) -> Vec<String> {
    if !config.subrepos.is_empty() {
        return config
            .subrepos
            .iter()
            .filter(|s| s.path != ".")
            .map(|s| s.path.clone())
            .collect();
    }
    if root.join(".harness").is_dir() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with('.')
            && !matches!(name, "node_modules" | "target")
            && path.join(".harness/config.json").is_file()
        {
            dirs.push(name.to_owned());
        }
    }
    dirs.sort();
    dirs
}
