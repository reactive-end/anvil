//! PreToolUse security guardrails: destructive shell prompts, secret files, and generated Godot metadata.
//!
//! The guard is a safety net against accidents, not a sandbox: it inspects command text only, so
//! scripts on disk and dynamically built commands remain the job of the host permission system.

use std::path::Path;

use anvil_adapters::contract::{HookOutcome, NormalizedHookInput};
use anvil_core::config::security_guard::{
    GuardCategory, GuardDecision, PatternList, SecurityGuardConfig,
};

use super::payload::{extract_rel_file, is_mutating_file_tool, strip_tool_namespace};
use super::shell::{Risk, destructive, expand, secrets};
use inputs::{GuardText, TextKind};

/// What the guard decided about one tool call, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub decision: GuardDecision,
    /// Category of the risk that drove the decision; absent for patterns and fixed rules.
    pub category: Option<GuardCategory>,
    /// Message shown to the user (empty when nothing was found).
    pub reason: String,
}

impl Verdict {
    fn new(decision: GuardDecision, category: Option<GuardCategory>, reason: String) -> Self {
        Self { decision, category, reason }
    }

    fn allow() -> Self {
        Self::new(GuardDecision::Allow, None, String::new())
    }
}

/// A risk plus the text it was found in.
struct Found {
    risk: Risk,
    subject: String,
}

/// Evaluates `PreToolUse` security guardrails before tool execution.
pub fn check_security_guard(
    root: &Path,
    config: &SecurityGuardConfig,
    input: &NormalizedHookInput,
) -> Option<HookOutcome> {
    let verdict = evaluate(root, config, input);
    match verdict.decision {
        GuardDecision::Allow => None,
        GuardDecision::Ask => Some(HookOutcome::ask(verdict.reason)),
        GuardDecision::Deny => Some(HookOutcome::deny(verdict.reason)),
    }
}

/// Decides what happens to `input`: deny patterns, then self-protection and fixed rules, then allow
/// patterns, ask patterns, and finally the decision of the risk category.
pub fn evaluate(root: &Path, config: &SecurityGuardConfig, input: &NormalizedHookInput) -> Verdict {
    let texts = inputs::guard_texts(input);
    let raw: Vec<&str> = texts.iter().map(|t| t.text).collect();
    if let Some(pattern) = config.first_match(PatternList::Deny, &raw) {
        return Verdict::new(
            GuardDecision::Deny,
            None,
            format!(
                "Security Guard blocked a command matching the deny pattern `{pattern}`. Run it yourself, or drop the pattern with `anvil guard remove '{pattern}'`."
            ),
        );
    }
    if let Some(what) = inputs::protects_config(root, input, &texts) {
        return Verdict::new(
            GuardDecision::Ask,
            None,
            format!(
                "Security Guard requires confirmation before {what}; only the user should relax the guard."
            ),
        );
    }
    if let Some(blocked) = generated_file_block(root, input) {
        return blocked;
    }
    if let Some(pattern) = config.first_match(PatternList::Allow, &raw) {
        let reason = format!("matches the allow pattern `{pattern}`");
        return Verdict::new(GuardDecision::Allow, None, reason);
    }
    if let Some(pattern) = config.first_match(PatternList::Ask, &raw) {
        let reason = format!(
            "Security Guard requires confirmation: the command matches the ask pattern `{pattern}`."
        );
        return Verdict::new(GuardDecision::Ask, None, reason);
    }
    strictest(config, find_risks(root, input, &texts))
        .map_or_else(Verdict::allow, |found| verdict_for(config, &found))
}

/// Builds the `tool_input` that makes `text` reach the guard the way `tool_name` would send it:
/// MCP tools get both a shell-like and a SQL-like field, file tools a path, others a command.
pub fn tool_input_for(tool_name: &str, text: &str) -> serde_json::Value {
    if inputs::is_mcp_tool(Some(tool_name)) {
        return serde_json::json!({ "cli_command": text, "sql": text });
    }
    let is_file_tool = is_file_read_tool(Some(tool_name)) || is_mutating_file_tool(Some(tool_name));
    if is_file_tool {
        return serde_json::json!({ "file_path": text });
    }
    serde_json::json!({ "command": text })
}

/// Returns `true` when any git invocation reachable from `cmd` disables repository hooks.
pub fn bypasses_git_hooks(cmd: &str) -> bool {
    expand::expand(cmd).invocations.iter().any(destructive::bypasses_git_hooks)
}

fn generated_file_block(root: &Path, input: &NormalizedHookInput) -> Option<Verdict> {
    if !is_mutating_file_tool(input.tool_name.as_deref()) {
        return None;
    }
    let rel = extract_rel_file(root, input.tool_input.as_ref())?;
    is_godot_generated_file(&rel).then(|| {
        Verdict::new(
            GuardDecision::Deny,
            None,
            format!(
                "Security Guard blocked direct edit to Godot-generated metadata `{rel}`. Run `godot --headless --import` instead."
            ),
        )
    })
}

fn find_risks(root: &Path, input: &NormalizedHookInput, texts: &[GuardText<'_>]) -> Vec<Found> {
    let mut found = Vec::new();
    for text in texts {
        let risks = match text.kind {
            TextKind::Shell => describe_shell(root, text.text),
            TextKind::Query => destructive::describe_query(text.text).into_iter().collect(),
        };
        let subject = snippet(text.text);
        found.extend(risks.into_iter().map(|risk| Found { risk, subject: subject.clone() }));
    }
    found.extend(secret_file_risk(root, input));
    found
}

fn describe_shell(root: &Path, cmd: &str) -> Vec<Risk> {
    let expansion = expand::expand(cmd);
    let secret = |desc| Risk::new(GuardCategory::Secrets, desc);
    let mut risks: Vec<Risk> = expansion
        .invocations
        .iter()
        .filter_map(|inv| {
            destructive::describe(root, inv).or_else(|| secrets::describe(inv).map(secret))
        })
        .collect();
    risks.extend(expansion.inline_code.iter().filter_map(|code| {
        destructive::describe_code(root, code).or_else(|| secrets::describe_code(code).map(secret))
    }));
    risks.extend(expansion.opaque.map(|desc| Risk::new(GuardCategory::Opaque, desc)));
    risks
}

fn secret_file_risk(root: &Path, input: &NormalizedHookInput) -> Option<Found> {
    let touches_file = is_file_read_tool(input.tool_name.as_deref())
        || is_mutating_file_tool(input.tool_name.as_deref());
    let rel = touches_file.then(|| extract_rel_file(root, input.tool_input.as_ref()))??;
    secrets::is_secret_file(&rel).then(|| Found {
        risk: Risk::new(GuardCategory::Secrets, "accessing a secret/credential file"),
        subject: rel,
    })
}

/// The risk whose configured decision is the most restrictive; the first one wins ties.
fn strictest(config: &SecurityGuardConfig, found: Vec<Found>) -> Option<Found> {
    let rank = |f: &Found| match config.decision(f.risk.category) {
        GuardDecision::Deny => 2,
        GuardDecision::Ask => 1,
        GuardDecision::Allow => 0,
    };
    found.into_iter().fold(None, |best, next| match best {
        Some(current) if rank(&current) >= rank(&next) => Some(current),
        _ => Some(next),
    })
}

fn verdict_for(config: &SecurityGuardConfig, found: &Found) -> Verdict {
    let category = found.risk.category;
    let name = category.as_str();
    let (desc, subject) = (found.risk.desc, &found.subject);
    let decision = config.decision(category);
    let reason = match decision {
        GuardDecision::Allow => format!("the `{name}` category is set to allow ({desc})"),
        GuardDecision::Ask => {
            format!("Security Guard requires confirmation before {desc} (`{subject}`).")
        }
        GuardDecision::Deny => format!(
            "Security Guard blocked {desc} (`{subject}`). Run it yourself, or relax it with `anvil guard set {name} ask`."
        ),
    };
    Verdict::new(decision, Some(category), reason)
}

/// Shortens `text` for messages: one line, at most 100 characters, no backticks.
fn snippet(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ").replace('`', "'");
    match flat.char_indices().nth(100) {
        Some((cut, _)) => format!("{}...", &flat[..cut]),
        None => flat,
    }
}

fn is_file_read_tool(tool_name: Option<&str>) -> bool {
    strip_tool_namespace(tool_name).is_some_and(|bare| {
        matches!(bare, "read_file" | "Read" | "view_file" | "Grep" | "grep_search")
    })
}

fn is_godot_generated_file(rel: &str) -> bool {
    let lower = rel.replace('\\', "/").to_ascii_lowercase();
    lower.starts_with(".godot/") || lower.contains("/.godot/") || lower.ends_with(".import")
}

mod inputs;

#[cfg(test)]
mod tests;
