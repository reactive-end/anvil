//! PreToolUse security guardrails: destructive shell prompts, secret files, and generated Godot metadata.

use std::path::Path;

use anvil_adapters::contract::{HookOutcome, NormalizedHookInput};

use super::payload::{
    extract_command_field, extract_rel_file, is_mutating_file_tool, strip_tool_namespace,
};

const DESTRUCTIVE_SHELL_PATTERNS: &[(&str, &str)] = &[
    ("git reset --hard", "destructive git reset (`git reset --hard`)"),
    ("git checkout -- .", "discarding all working tree changes (`git checkout -- .`)"),
    ("git clean -f", "force-deleting untracked files (`git clean -f`)"),
    ("git clean -df", "force-deleting untracked files/dirs (`git clean -df`)"),
    ("git clean -fd", "force-deleting untracked files/dirs (`git clean -fd`)"),
    ("git push --force", "force pushing (`git push --force`)"),
    ("git push -f", "force pushing (`git push -f`)"),
    ("migrate:fresh", "destructive database wipe (`migrate:fresh`)"),
    ("db:wipe", "destructive database wipe (`db:wipe`)"),
    ("prisma migrate reset", "destructive database reset (`prisma migrate reset`)"),
];

const DESTRUCTIVE_SQL_PATTERNS: &[(&str, &str)] = &[
    ("drop database", "destructive SQL (`DROP DATABASE`)"),
    ("drop table", "destructive SQL (`DROP TABLE`)"),
    ("truncate table", "destructive SQL (`TRUNCATE TABLE`)"),
];

const SECRET_READ_CMDS: &[&str] = &[
    "cat",
    "type",
    "get-content",
    "gc",
    "head",
    "tail",
    "less",
    "more",
    "sed",
    "awk",
    "base64",
    "cp",
    "copy",
    "mv",
    "move",
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
    let unquoted = normalize_ws(&strip_quoted_segments(cmd));
    if let Some((_, desc)) = DESTRUCTIVE_SHELL_PATTERNS
        .iter()
        .find(|(pat, _)| unquoted.contains(pat))
    {
        return Some(*desc);
    }
    if let Some(desc) = match_dangerous_rm(&unquoted) {
        return Some(desc);
    }
    if match_shell_secret_read(cmd) {
        return Some("reading or copying a secret/credential file from shell");
    }
    let full = normalize_ws(cmd);
    let non_db_cmd = ["git commit", "echo ", "rg ", "grep ", "select-string "]
        .iter()
        .any(|p| full.starts_with(p));
    if non_db_cmd {
        return None;
    }
    DESTRUCTIVE_SQL_PATTERNS
        .iter()
        .find(|(pat, _)| full.contains(pat))
        .map(|(_, desc)| *desc)
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn strip_quoted_segments(cmd: &str) -> String {
    let mut out = String::with_capacity(cmd.len());
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in cmd.chars() {
        if escaped {
            escaped = false;
            if quote.is_none() {
                out.push(ch);
            }
            continue;
        }
        if ch == '\\' && quote == Some('"') {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
                out.push(' ');
            }
            continue;
        }
        if matches!(ch, '"' | '\'') {
            quote = Some(ch);
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

fn match_dangerous_rm(unquoted_lower: &str) -> Option<&'static str> {
    let tokens: Vec<&str> = unquoted_lower.split_whitespace().collect();
    for (idx, tok) in tokens.iter().enumerate() {
        if *tok != "rm" && !tok.ends_with("/rm") {
            continue;
        }
        let mut recursive = false;
        let mut force = false;
        for arg in tokens.iter().skip(idx + 1) {
            if matches!(*arg, "&&" | "||" | "|" | ";") {
                break;
            }
            if *arg == "--recursive" {
                recursive = true;
            } else if *arg == "--force" {
                force = true;
            } else if arg.starts_with('-') && !arg.starts_with("--") {
                recursive |= arg.contains('r');
                force |= arg.contains('f');
            } else if recursive && force {
                if *arg == "/" || arg.starts_with("/*") {
                    return Some("recursive deletion near filesystem root (`rm -rf /`)");
                }
                if matches!(*arg, "." | "./" | ".." | "../" | "*") {
                    return Some("recursive deletion of working directory (`rm -rf .`)");
                }
                if *arg == "~" || arg.starts_with("~/") {
                    return Some("recursive deletion of home directory (`rm -rf ~`)");
                }
            }
        }
    }
    None
}

fn match_shell_secret_read(cmd: &str) -> bool {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let Some((pos, _)) = tokens.iter().enumerate().find(|(_, t)| {
        let bare = t.rsplit(['/', '\\']).next().unwrap_or(t).trim_end_matches(".exe");
        !bare.contains('=') && SECRET_READ_CMDS.contains(&bare.to_ascii_lowercase().as_str())
    }) else {
        return false;
    };
    tokens.iter().skip(pos + 1).any(|arg| {
        if matches!(*arg, "&&" | "||" | "|" | ";") {
            return false;
        }
        if arg.starts_with('-') {
            return false;
        }
        let clean = arg.trim_matches(|c| matches!(c, '"' | '\'' | '<' | '>'));
        !clean.is_empty() && is_secret_file(clean)
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_security_guard_avoids_quoted_false_positives_and_catches_evasion() {
        assert!(match_destructive_command("git commit -m \"fix git reset --hard\"").is_none());
        assert!(match_destructive_command("rm -rf ./").is_some());
        assert!(match_destructive_command("rm -fr /").is_some());
        assert!(match_destructive_command("rm -r -f ~").is_some());
        assert!(match_destructive_command("cat .env").is_some());
        assert!(match_destructive_command("cat .env.example").is_none());
        assert!(match_destructive_command("psql -c \"DROP TABLE users\"").is_some());
    }
}
