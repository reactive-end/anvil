//! PreToolUse security guardrails: destructive shell prompts, secret files, and generated Godot metadata.

use std::path::Path;

use anvil_adapters::contract::{HookOutcome, NormalizedHookInput};

use super::payload::{
    extract_command_field, extract_rel_file, is_mutating_file_tool, strip_tool_namespace,
};

const DESTRUCTIVE_CMD_PATTERNS: &[(&str, &str)] = &[
    ("git reset --hard", "destructive git reset (`git reset --hard`)"),
    ("git checkout -- .", "discarding all working tree changes (`git checkout -- .`)"),
    ("git clean -f", "force-deleting untracked files (`git clean -f`)"),
    ("git clean -df", "force-deleting untracked files/dirs (`git clean -df`)"),
    ("git clean -fd", "force-deleting untracked files/dirs (`git clean -fd`)"),
    ("git push --force", "force pushing (`git push --force`)"),
    ("git push -f", "force pushing (`git push -f`)"),
    ("rm -rf /", "recursive deletion near filesystem root (`rm -rf /`)"),
    ("rm -rf .", "recursive deletion of working directory (`rm -rf .`)"),
    ("rm -rf ~", "recursive deletion of home directory (`rm -rf ~`)"),
    ("drop database", "destructive SQL (`DROP DATABASE`)"),
    ("drop table", "destructive SQL (`DROP TABLE`)"),
    ("truncate table", "destructive SQL (`TRUNCATE TABLE`)"),
    ("migrate:fresh", "destructive database wipe (`migrate:fresh`)"),
    ("db:wipe", "destructive database wipe (`db:wipe`)"),
    ("prisma migrate reset", "destructive database reset (`prisma migrate reset`)"),
];

const SAFE_ENV_TEMPLATES: &[&str] = &[
    ".env.example",
    ".env.sample",
    ".env.template",
    ".env.dist",
    ".env.schema",
];

/// Evaluates `PreToolUse` security guardrails before tool execution.
pub fn check_security_guard(root: &Path, input: &NormalizedHookInput) -> Option<HookOutcome> {
    if let Some((_, cmd)) = extract_command_field(input.tool_input.as_ref())
        && let Some(desc) = match_destructive_command(cmd)
    {
        return Some(HookOutcome::ask(format!(
            "Security Guard requires confirmation before {desc}."
        )));
    }

    let is_read = is_file_read_tool(input.tool_name.as_deref());
    let is_write = is_mutating_file_tool(input.tool_name.as_deref());
    if !is_read && !is_write {
        return None;
    }

    let rel = extract_rel_file(root, input.tool_input.as_ref())?;
    if is_write && is_godot_generated_file(&rel) {
        return Some(HookOutcome::deny(format!(
            "Security Guard blocked direct edit to Godot-generated metadata `{rel}`. Run `godot --headless --import` instead."
        )));
    }
    if is_secret_file(&rel) {
        return Some(HookOutcome::ask(format!(
            "Security Guard requires confirmation before accessing secret/credential file `{rel}`."
        )));
    }
    None
}

fn match_destructive_command(cmd: &str) -> Option<&'static str> {
    let lower = cmd
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    DESTRUCTIVE_CMD_PATTERNS
        .iter()
        .find(|(pat, _)| lower.contains(pat))
        .map(|(_, desc)| *desc)
}

fn is_file_read_tool(tool_name: Option<&str>) -> bool {
    strip_tool_namespace(tool_name)
        .is_some_and(|bare| matches!(bare, "read_file" | "Read" | "view_file"))
}

fn is_godot_generated_file(rel: &str) -> bool {
    let lower = rel.replace('\\', "/").to_ascii_lowercase();
    lower.starts_with(".godot/")
        || lower.contains("/.godot/")
        || lower.ends_with(".import")
}

fn is_secret_file(rel: &str) -> bool {
    let norm = rel.replace('\\', "/").to_ascii_lowercase();
    let name = norm.rsplit('/').next().unwrap_or(&norm);
    if SAFE_ENV_TEMPLATES.contains(&name) {
        return false;
    }
    name == ".env"
        || name.starts_with(".env.")
        || name == ".npmrc"
        || name == ".pypirc"
        || name == "credentials.json"
        || name == "id_rsa"
        || name == "id_ed25519"
        || name.ends_with(".pem")
        || name.ends_with(".key")
}
