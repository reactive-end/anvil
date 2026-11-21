//! `anvil hook`: unified Antigravity & Claude Code hook dispatcher.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use anyhow::Result;
use anvil_adapters::contract::{
    AdapterKind, HookEvent, HookOutcome, InboxMessage, MessageLevel, NormalizedHookInput,
};
use anvil_adapters::{antigravity, claude_code};
use anvil_ast::analyze_file;
use anvil_core::circuit_breaker::CircuitBreaker;
use anvil_core::config::AnvilConfig;
use anvil_core::diff_classifier::{ChangedFile, classify_files};
use anvil_core::git::{self, DiffScope};
use anvil_core::traits::Severity;
use anvil_graph::CodeGraphEngine;
use anvil_graph::builder::update_file;
use anvil_graph::model::CodeGraph;
use anvil_graph::store;
use anvil_memory::MemoryEngine;
use anvil_tasks::{list_tasks, resolve_author};
use serde_json::Value;

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
        HookEvent::PreToolUse => handle_pre_tool_use(&effective_root, &config, &input),
        HookEvent::PostToolUse => handle_post_tool_use(&effective_root, &config, &input),
        HookEvent::PreInvocation => HookOutcome::allow(),
        HookEvent::Stop => handle_stop(&effective_root, &config, is_continuation_active(&raw_stdin)),
    };

    let rendered = match adapter {
        AdapterKind::Antigravity => antigravity::render_output(&effective_root, event, &outcome)?,
        AdapterKind::ClaudeCode => claude_code::render_output(&effective_root, event, &outcome)?,
    };
    println!("{}", serde_json::to_string(&rendered)?);
    Ok(())
}

fn resolve_hook_root(default_root: &Path, cwd: Option<&str>) -> PathBuf {
    let Some(raw) = cwd.map(str::trim).filter(|s| !s.is_empty()) else {
        return default_root.to_path_buf();
    };
    let path = PathBuf::from(raw);
    if path.is_dir() { git::repo_root(&path).unwrap_or(path) } else { default_root.to_path_buf() }
}

fn is_continuation_active(raw_json: &str) -> bool {
    let Ok(val) = serde_json::from_str::<Value>(raw_json) else { return false; };
    ["continuationActive", "continuation_active", "stop_hook_active"]
        .iter()
        .any(|k| val.get(*k).and_then(Value::as_bool) == Some(true))
}

fn handle_session_start(root: &Path, config: &AnvilConfig) -> HookOutcome {
    let mut messages = Vec::new();
    let author = resolve_author(root).unwrap_or_else(|_| "hook".to_owned());
    if let Ok(mut mem) = MemoryEngine::open(root, author) {
        let _ = mem.sync();
        if let Ok(hits) = mem.session_context(450) && !hits.is_empty() {
            let summary: Vec<String> = hits.into_iter().map(|h| format!("[{}] {}", h.kind, h.title)).collect();
            messages.push(InboxMessage::new("anvil-memory", MessageLevel::Info, summary.join("; "), None));
        }
    }
    if let Ok(g) = store::load_binary(&store::default_bin_path(root)) {
        let msg = format!("Graph ready ({} nodes, {} edges)", g.nodes.len(), g.edges.len());
        messages.push(InboxMessage::new("anvil-graph", MessageLevel::Info, msg, None));
    }
    if let Ok(tasks) = list_tasks(root) && let Some(latest) = tasks.last() {
        let msg = format!("Latest task {}: {}", latest.task_id, latest.request);
        messages.push(InboxMessage::new("anvil-tasks", MessageLevel::Info, msg, None));
    }
    if let Ok(st) = CircuitBreaker::new(root, config.circuit_breaker_limit).status() && st.is_open() {
        let msg = "Circuit Breaker is OPEN; run `anvil circuit reset` after fixing.".to_owned();
        messages.push(InboxMessage::new("circuit-breaker", MessageLevel::Error, msg, None));
    }
    HookOutcome::with_messages(messages)
}

fn handle_pre_tool_use(root: &Path, config: &AnvilConfig, input: &NormalizedHookInput) -> HookOutcome {
    if let Ok(st) = CircuitBreaker::new(root, config.circuit_breaker_limit).status() && st.is_open() {
        let tool = input.tool_name.as_deref().unwrap_or("tool");
        return HookOutcome::deny(format!(
            "Circuit Breaker OPEN ({} identical failures) — blocked `{tool}`. Run `anvil circuit reset` after fixing.",
            st.consecutive_failures
        ));
    }
    if let Some((key, cmd)) = extract_command_field(input.tool_input.as_ref()) {
        if cmd.contains("--no-verify") {
            return HookOutcome::deny("Forbidden `--no-verify` bypass flag detected.");
        }
        if should_wrap_with_anvil_run(cmd)
            && let Some(mut obj) = input.tool_input.as_ref().and_then(Value::as_object).cloned()
        {
            obj.insert(key.to_owned(), Value::String(format!("anvil run -- {cmd}")));
            return HookOutcome::overwrite(Value::Object(obj), Some("Wrapped command with `anvil run`".to_owned()));
        }
    }
    HookOutcome::allow()
}

fn handle_post_tool_use(root: &Path, config: &AnvilConfig, input: &NormalizedHookInput) -> HookOutcome {
    let mut messages = Vec::new();
    if let Some(rel) = extract_rel_file(root, input.tool_input.as_ref()) {
        collect_tier0_messages(root, &rel, config, &mut messages);
        let bin_path = store::default_bin_path(root);
        if bin_path.is_file()
            && let Ok(mut graph) = store::load_binary(&bin_path)
            && matches!(update_file(&mut graph, root, &rel, config), Ok(true))
        {
            let _ = store::save_binary(&graph, &bin_path);
        }
        if let Ok(mut mem) = MemoryEngine::open(root, "hook") {
            let _ = mem.mark_files_changed(std::slice::from_ref(&rel));
        }
    }
    update_circuit_from_output(root, config, input, &mut messages);
    HookOutcome::with_messages(messages)
}

fn collect_tier0_messages(root: &Path, rel: &str, config: &AnvilConfig, messages: &mut Vec<InboxMessage>) {
    let abs = root.join(rel);
    let Ok(source) = fs::read_to_string(&abs) else { return; };
    let Ok(facts) = analyze_file(Path::new(rel), &source, config) else { return; };
    for d in facts.diagnostics {
        let lvl = match d.severity {
            Severity::Error => MessageLevel::Error,
            Severity::Warning => MessageLevel::Warning,
            Severity::Info => MessageLevel::Info,
        };
        messages.push(InboxMessage::new(
            format!("tier0:{}", d.rule),
            lvl,
            format!("L{}: {}", d.line, d.message),
            Some(rel.to_owned()),
        ));
    }
}

fn handle_stop(root: &Path, config: &AnvilConfig, continuation_active: bool) -> HookOutcome {
    if continuation_active {
        return HookOutcome::allow();
    }
    if let Ok(st) = CircuitBreaker::new(root, config.circuit_breaker_limit).status() && st.is_open() {
        return HookOutcome::with_messages(vec![InboxMessage::new(
            "circuit-breaker",
            MessageLevel::Error,
            "Circuit Breaker OPEN — stopping for human escalation.".to_owned(),
            None,
        )]);
    }
    let changed = git::changed_files(root, DiffScope::WorkingTree).unwrap_or_default();
    if changed.is_empty() {
        return HookOutcome::allow();
    }
    let errs = collect_changed_tier0_errors(root, &changed, config);
    if !errs.is_empty() {
        return HookOutcome::block_stop(format!("Tier 0 AST gate failed ({} error(s)): {}", errs.len(), errs.join("; ")));
    }
    let mut messages = Vec::new();
    if let Ok(engine) = CodeGraphEngine::load_or_build(root, config.clone()) {
        let blast = engine.blast_radius(&changed, 2);
        if !blast.impacted_files.is_empty() {
            let msg = format!("{} impacted file(s), {} related test(s)", blast.impacted_files.len(), blast.related_tests.len());
            messages.push(InboxMessage::new("tier1:impact", MessageLevel::Info, msg, None));
        }
    }
    let classified = classify_files(&changed.iter().map(ChangedFile::from_path).collect::<Vec<_>>());
    if classified.needs_visual_qa {
        messages.push(InboxMessage::new("tier2:qa", MessageLevel::Warning, "UI files changed; verify with `anvil qa`.".to_owned(), None));
    }
    HookOutcome::with_messages(messages)
}

fn collect_changed_tier0_errors(root: &Path, changed: &[String], config: &AnvilConfig) -> Vec<String> {
    let mut errs = Vec::new();
    for rel in changed {
        if let Ok(src) = fs::read_to_string(root.join(rel))
            && let Ok(facts) = analyze_file(Path::new(rel), &src, config)
        {
            errs.extend(
                facts.diagnostics.into_iter()
                    .filter(|d| d.severity == Severity::Error)
                    .map(|d| format!("{}:{} [{}] {}", d.file, d.line, d.rule, d.message)),
            );
        }
    }
    errs
}

fn update_circuit_from_output(root: &Path, config: &AnvilConfig, input: &NormalizedHookInput, messages: &mut Vec<InboxMessage>) {
    let Some(out) = &input.tool_output else { return; };
    let code = out.get("exitCode").or_else(|| out.get("exit_code")).and_then(Value::as_i64);
    let cb = CircuitBreaker::new(root, config.circuit_breaker_limit);
    match code {
        Some(0) => { let _ = cb.record_success(); }
        Some(c) => {
            let tool = input.tool_name.as_deref().unwrap_or("command");
            let snippet = out.get("stderr").or_else(|| out.get("output")).and_then(Value::as_str).unwrap_or("non-zero exit");
            let cmd = extract_command_field(input.tool_input.as_ref()).map(|(_, s)| s).unwrap_or(tool);
            if let Ok(st) = cb.record_failure(tool, &format!("exit {c}: {snippet}"), cmd) && st.is_open() {
                messages.push(InboxMessage::new(
                    "circuit-breaker",
                    MessageLevel::Error,
                    "Circuit Breaker tripped OPEN after 3 identical failures; run `anvil circuit reset`.".to_owned(),
                    None,
                ));
            }
        }
        None => {}
    }
}

fn extract_command_field(tool_input: Option<&Value>) -> Option<(&'static str, &str)> {
    let obj = tool_input?.as_object()?;
    ["CommandLine", "command", "cmd"]
        .into_iter()
        .find_map(|k| obj.get(k).and_then(Value::as_str).map(|s| (k, s.trim())))
}

fn should_wrap_with_anvil_run(cmd: &str) -> bool {
    if cmd.starts_with("anvil ") || cmd.contains("anvil run") {
        return false;
    }
    ["tsc", "npx tsc", "eslint", "npx eslint", "biome", "vitest", "npx vitest", "jest", "npx jest", "pytest", "ruff", "dart analyze", "flutter test", "./gradlew", "gradlew"]
        .iter()
        .any(|p| cmd.starts_with(p))
}

fn extract_rel_file(root: &Path, tool_input: Option<&Value>) -> Option<String> {
    let obj = tool_input?.as_object()?;
    let raw = ["TargetFile", "file_path", "path", "AbsolutePath", "target_file"]
        .iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_str))?;
    let p = Path::new(raw);
    let rel = if p.is_absolute() {
        p.strip_prefix(root).ok().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| raw.to_owned())
    } else {
        raw.to_owned()
    };
    let norm = CodeGraph::normalize_rel_path(&rel);
    (!norm.is_empty()).then_some(norm)
}
