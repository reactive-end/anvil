//! Tier 0 AST, promoted ratchet rule, and Circuit Breaker helpers for `anvil hook`.

use std::path::Path;

use anvil_adapters::contract::{InboxMessage, MessageLevel, NormalizedHookInput};
use anvil_ast::analyze_file;
use anvil_ast::rules::{has_new_code_only_findings, retain_new_code_findings};
use anvil_core::circuit_breaker::CircuitBreaker;
use anvil_core::config::AnvilConfig;
use anvil_core::ratchet::{compile_promoted_rules, evaluate_compiled, load_promoted_rules};
use anvil_core::stats::{StatsEvent, record_event};
use anvil_core::test_tampering::{check_test_tampering_diff, is_test_file};
use anvil_core::tier0_cache::{Tier0Cache, compute_rules_hash};
use anvil_core::traits::{Diagnostic, Severity};
use serde_json::Value;

use super::payload::{extract_command_field, is_anvil_run_command};
use crate::commands::check::read_bounded_source;

/// Collects Tier 0 AST and promoted ratchet diagnostics for a single modified file.
pub fn collect_post_tool_messages(
    root: &Path,
    rel: &str,
    config: &AnvilConfig,
    messages: &mut Vec<InboxMessage>,
) {
    if !root.join(".harness").is_dir() {
        return;
    }
    let Some(source) = read_bounded_source(&root.join(rel)) else {
        return;
    };
    let mut errs = Vec::new();
    if let Ok(facts) = analyze_file(Path::new(rel), &source, config) {
        let mut diagnostics = facts.diagnostics;
        if has_new_code_only_findings(&diagnostics) {
            let added = anvil_core::git::added_line_ranges(root, false).ok();
            retain_new_code_findings(rel, &mut diagnostics, added.as_ref());
        }
        push_diagnostics(root, "tier0", rel, diagnostics, messages, &mut errs);
    }
    if let Ok(rules) = load_promoted_rules(root)
        && !rules.is_empty()
    {
        let compiled = compile_promoted_rules(&rules);
        if !compiled.is_empty() {
            let diags = evaluate_compiled(&compiled, rel, &source);
            push_diagnostics(root, "ratchet", rel, diags, messages, &mut errs);
        }
    }

    let cb = CircuitBreaker::new(root, config.circuit_breaker_limit);
    if errs.is_empty() {
        let _ = cb.record_command_success("tier0-ast");
    } else if let Ok(st) = cb.record_failure("tier0-ast", &errs.join("; "), "tier0-ast")
        && st.is_open()
    {
        messages.push(InboxMessage::new(
            "circuit-breaker",
            MessageLevel::Error,
            "Circuit Breaker tripped OPEN after repeated Tier 0 AST failures; run `anvil circuit reset`.",
            Some(rel.to_owned()),
        ));
    }
}

/// Collects blocking `Severity::Error` messages from Tier 0 AST and promoted ratchet rules,
/// skipping files that already hit the `(mtime, len, rules_hash)` clean cache.
pub fn collect_changed_gate_errors(
    root: &Path,
    changed: &[String],
    config: &AnvilConfig,
) -> Vec<String> {
    let rules = load_promoted_rules(root).unwrap_or_default();
    let compiled = compile_promoted_rules(&rules);
    let rules_hash = compute_rules_hash(root, config);
    let mut cache = Tier0Cache::load(root);
    let mut cache_dirty = false;
    let mut errs = Vec::new();
    let mut added_lines = None;
    let has_test_changes = changed.iter().any(|p| is_test_file(p));
    let test_diffs = if has_test_changes {
        anvil_core::git::pending_diffs(root).unwrap_or_default()
    } else {
        Default::default()
    };

    for rel in changed {
        if let Some(diff_text) = test_diffs.get(rel) {
            extend_error_lines(&mut errs, check_test_tampering_diff(rel, diff_text));
        }
        let abs = root.join(rel);
        if cache.is_clean_hit(&abs, rel, &rules_hash) {
            continue;
        }
        let Some(src) = read_bounded_source(&abs) else {
            continue;
        };
        let before = errs.len();
        if let Ok(facts) = analyze_file(Path::new(rel), &src, config) {
            let mut diagnostics = facts.diagnostics;
            if has_new_code_only_findings(&diagnostics) {
                let added = added_lines
                    .get_or_insert_with(|| anvil_core::git::added_line_ranges(root, false).ok());
                retain_new_code_findings(rel, &mut diagnostics, added.as_ref());
            }
            extend_error_lines(&mut errs, diagnostics);
        }
        if !compiled.is_empty() {
            extend_error_lines(&mut errs, evaluate_compiled(&compiled, rel, &src));
        }
        if errs.len() == before {
            cache.record_clean(&abs, rel, &rules_hash);
        } else {
            cache.invalidate(rel);
        }
        cache_dirty = true;
    }

    if cache_dirty && root.join(".harness").is_dir() {
        let _ = cache.save(root);
    }
    errs
}

/// Updates the `N=3` Circuit Breaker from `PostToolUse` command exit codes.
pub fn update_circuit_from_output(
    root: &Path,
    config: &AnvilConfig,
    input: &NormalizedHookInput,
    messages: &mut Vec<InboxMessage>,
) {
    if !root.join(".harness").is_dir() {
        return;
    }
    let Some(out) = &input.tool_output else {
        return;
    };
    let code = out.get("exitCode").or_else(|| out.get("exit_code")).and_then(Value::as_i64);
    let tool = input.tool_name.as_deref().unwrap_or("command");
    let cmd = extract_command_field(input.tool_input.as_ref()).map(|(_, s)| s).unwrap_or(tool);
    let cb = CircuitBreaker::new(root, config.circuit_breaker_limit);
    if let Some(c) = code
        && root.join(".harness").is_dir()
    {
        let _ = anvil_tasks::record_active_command_outcome(root, cmd, c == 0);
    }
    if is_anvil_run_command(cmd) {
        if matches!(code, Some(c) if c != 0)
            && let Ok(st) = cb.status()
            && st.is_open()
        {
            push_circuit_open_msg(messages, st.consecutive_failures);
        }
        return;
    }

    match code {
        Some(0) => {
            let _ = cb.record_command_success(cmd);
        }
        Some(c) => {
            let snippet = ["stderr", "output", "stdout"]
                .iter()
                .filter_map(|k| out.get(*k).and_then(Value::as_str))
                .map(str::trim)
                .find(|s| !s.is_empty())
                .unwrap_or("non-zero exit");
            if let Ok(st) = cb.record_failure(tool, &format!("exit {c}: {snippet}"), cmd)
                && st.is_open()
            {
                push_circuit_open_msg(messages, st.consecutive_failures);
            }
        }
        None => {}
    }
}

fn push_circuit_open_msg(messages: &mut Vec<InboxMessage>, failures: u32) {
    let msg = format!(
        "Circuit Breaker tripped OPEN after {failures} identical failures; run `anvil circuit reset`."
    );
    messages.push(InboxMessage::new("circuit-breaker", MessageLevel::Error, msg, None));
}

fn push_diagnostics(
    root: &Path,
    prefix: &str,
    rel: &str,
    diagnostics: Vec<Diagnostic>,
    messages: &mut Vec<InboxMessage>,
    errs: &mut Vec<String>,
) {
    for d in diagnostics {
        let _ = record_event(root, StatsEvent::RuleHit { rule: d.rule.clone() });
        if d.severity == Severity::Error {
            errs.push(format!("{}:{} [{}] {}", d.file, d.line, d.rule, d.message));
        }
        let lvl = match d.severity {
            Severity::Error => MessageLevel::Error,
            Severity::Warning => MessageLevel::Warning,
            Severity::Info => MessageLevel::Info,
        };
        let msg = format!("L{}: {}", d.line, d.message);
        messages.push(InboxMessage::new(
            format!("{prefix}:{}", d.rule),
            lvl,
            msg,
            Some(rel.to_owned()),
        ));
    }
}

fn extend_error_lines(errs: &mut Vec<String>, diagnostics: Vec<Diagnostic>) {
    errs.extend(
        diagnostics
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| format!("{}:{} [{}] {}", d.file, d.line, d.rule, d.message)),
    );
}
