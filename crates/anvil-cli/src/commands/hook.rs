//! `anvil hook`: unified Antigravity & Claude Code hook dispatcher.

mod gates;
mod payload;
mod task_auto;

use std::io::{self, Read};
use std::path::Path;

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
use anvil_graph::CodeGraphEngine;
use anvil_graph::builder::update_file;
use anvil_graph::store;
use anvil_memory::MemoryEngine;
use anvil_tasks::{list_tasks, resolve_author_fast};
use serde_json::Value;

use gates::{collect_changed_gate_errors, collect_post_tool_messages, update_circuit_from_output};
use payload::{
    extract_command_field, extract_rel_file, is_circuit_recovery_command, is_mutating_file_tool,
    is_read_only_tool, resolve_hook_root, should_bypass_stop, should_wrap_with_anvil_run,
};

/// Executes `anvil hook <event> --adapter <antigravity|claude>`.
pub fn execute(root: &Path, event_raw: &str, adapter_raw: &str) -> Result<()> {
    let adapter = AdapterKind::parse(adapter_raw)?;
    let event = HookEvent::parse(event_raw)?;
    let mut raw_stdin = String::new();
    io::stdin().read_to_string(&mut raw_stdin)?;

    let input = match adapter {
        AdapterKind::Antigravity => antigravity::parse_input(&raw_stdin, Some(event))?,
        AdapterKind::ClaudeCode => claude_code::parse_input(&raw_stdin, Some(event))?,
    };
    let effective_root = resolve_hook_root(root, input.cwd.as_deref());
    let config = AnvilConfig::load_or_default(&effective_root)?;

    let outcome = match event {
        HookEvent::SessionStart => handle_session_start(&effective_root, &config),
        HookEvent::UserPromptSubmit | HookEvent::PreInvocation => {
            task_auto::handle_prompt_hook(&effective_root, &input)
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
    Ok(())
}

fn handle_session_start(root: &Path, config: &AnvilConfig) -> HookOutcome {
    let mut messages = Vec::new();
    let author = resolve_author_fast(root).unwrap_or_else(|_| "hook".to_owned());
    if let Ok(mem) = MemoryEngine::open(root, author)
        && let Ok(hits) = mem.session_context(450)
        && !hits.is_empty()
    {
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
    if let Ok(header) = store::read_header(&store::default_bin_path(root)) {
        let msg = format!("Graph ready ({} nodes, {} edges)", header.live_nodes, header.live_edges);
        messages.push(InboxMessage::new("anvil-graph", MessageLevel::Info, msg, None));
    }
    if let Ok(tasks) = list_tasks(root)
        && let Some(latest) = tasks.last()
    {
        let msg = format!("Latest task {}: {}", latest.task_id, latest.request);
        messages.push(InboxMessage::new("anvil-tasks", MessageLevel::Info, msg, None));
    }
    if let Ok(st) = CircuitBreaker::new(root, config.circuit_breaker_limit).status()
        && st.is_open()
    {
        let msg = "Circuit Breaker is OPEN; run `anvil circuit reset` after fixing.".to_owned();
        messages.push(InboxMessage::new(
            "circuit-breaker",
            MessageLevel::Error,
            msg,
            None,
        ));
    }
    HookOutcome::with_messages(messages)
}

fn handle_pre_tool_use(
    root: &Path,
    config: &AnvilConfig,
    input: &NormalizedHookInput,
) -> HookOutcome {
    let cmd_field = extract_command_field(input.tool_input.as_ref());
    if let Ok(st) = CircuitBreaker::new(root, config.circuit_breaker_limit).status()
        && st.is_open()
    {
        let is_recovery = cmd_field.is_some_and(|(_, cmd)| is_circuit_recovery_command(cmd));
        if is_read_only_tool(input.tool_name.as_deref()) || is_recovery {
            return HookOutcome::allow();
        }
        let tool = input.tool_name.as_deref().unwrap_or("tool");
        return HookOutcome::deny(format!(
            "Circuit Breaker OPEN ({} identical failures) — blocked `{tool}`. Run `anvil circuit reset` after fixing.",
            st.consecutive_failures
        ));
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
        collect_post_tool_messages(root, &rel, config, &mut messages);
        let bin_path = store::default_bin_path(root);
        if bin_path.is_file() {
            let lock_path = root.join(".harness/.state/graph.lock");
            let _ = with_file_lock(&lock_path, 2000, || {
                if let Ok(mut graph) = store::load_binary(&bin_path)
                    && matches!(update_file(&mut graph, root, &rel, config), Ok(true))
                {
                    let _ = store::save_binary(&graph, &bin_path);
                }
                Ok(())
            });
        }
        if let Ok(mut mem) = MemoryEngine::open_without_sync(root, "hook") {
            let _ = mem.mark_files_changed(std::slice::from_ref(&rel));
        }
    }
    update_circuit_from_output(root, config, input, &mut messages);
    HookOutcome::with_messages(messages)
}

fn handle_stop(root: &Path, config: &AnvilConfig, bypass_stop: bool) -> HookOutcome {
    if bypass_stop {
        return HookOutcome::allow();
    }
    task_auto::refresh_tasks_json_if_changed(root);
    if let Ok(st) = CircuitBreaker::new(root, config.circuit_breaker_limit).status()
        && st.is_open()
    {
        let msg = "Circuit Breaker OPEN — stopping for human escalation.".to_owned();
        return HookOutcome::with_messages(vec![InboxMessage::new(
            "circuit-breaker",
            MessageLevel::Error,
            msg,
            None,
        )]);
    }
    let changed = git::changed_files(root, DiffScope::WorkingTree).unwrap_or_default();
    if changed.is_empty() {
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
    if let Ok(engine) = CodeGraphEngine::load_cached(root, config.clone()) {
        let blast = engine.blast_radius(&changed, 2);
        if !blast.impacted_files.is_empty() {
            let msg = format!(
                "{} impacted file(s), {} related test(s)",
                blast.impacted_files.len(),
                blast.related_tests.len()
            );
            messages.push(InboxMessage::new("tier1:impact", MessageLevel::Info, msg, None));
        }
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
    HookOutcome::with_messages(messages)
}
