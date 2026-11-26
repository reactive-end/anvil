//! Non-destructive adapter generator for `.codex/config.toml`.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::sync::{read_optional_string, write_if_changed};

const ANVIL_TOML_HEADER: &str = "[mcp_servers.anvil]";
const ANVIL_COMMAND_LINE: &str = r#"command = "anvil""#;
const ANVIL_ARGS_LINE: &str = r#"args = ["mcp"]"#;

/// Returns the path to `.codex/config.toml`.
pub fn config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".codex").join("config.toml")
}

/// Non-destructively ensures `[mcp_servers.anvil]` exists in `.codex/config.toml`.
pub fn merge_config_toml(repo_root: &Path) -> Result<bool> {
    let path = config_path(repo_root);
    let existing = read_optional_string(&path)?;
    let updated = upsert_toml_section(
        existing.as_deref(),
        ANVIL_TOML_HEADER,
        ANVIL_COMMAND_LINE,
        ANVIL_ARGS_LINE,
    );
    write_if_changed(&path, &updated)
}

/// Non-destructively merges per-subrepo `[mcp_servers.anvil-<slug>]` entries for Thin Umbrella roots.
pub fn merge_umbrella_config_toml(root: &Path, subrepos: &[(String, PathBuf)]) -> Result<bool> {
    let path = config_path(root);
    let existing = read_optional_string(&path)?;
    let mut updated = existing.clone().unwrap_or_default();
    for (slug, abs_path) in subrepos {
        let header = format!("[mcp_servers.anvil-{slug}]");
        let root_json = serde_json::to_string(&abs_path.to_string_lossy().into_owned())?;
        let args_line = format!(r#"args = ["--root", {root_json}, "mcp"]"#);
        updated = upsert_toml_section(Some(&updated), &header, ANVIL_COMMAND_LINE, &args_line);
    }
    if existing.is_none() && subrepos.is_empty() {
        return Ok(false);
    }
    write_if_changed(&path, &updated)
}

fn upsert_toml_section(
    existing: Option<&str>,
    header: &str,
    command_line: &str,
    args_line: &str,
) -> String {
    let block = format!("{header}\n{command_line}\n{args_line}\n");
    let Some(current) = existing.filter(|s| !s.trim().is_empty()) else {
        return block;
    };

    let lines: Vec<&str> = current.lines().collect();
    if let Some(header_idx) = lines.iter().position(|line| is_named_header(line, header)) {
        return update_existing_section(current, &lines, header_idx, command_line, args_line);
    }
    append_block(current, &block)
}

fn append_block(current: &str, block: &str) -> String {
    let sep = if current.ends_with("\n\n") || current.ends_with("\r\n\r\n") {
        ""
    } else if current.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{current}{sep}{block}")
}

fn update_existing_section(
    original: &str,
    lines: &[&str],
    header_idx: usize,
    command_line: &str,
    args_line: &str,
) -> String {
    let section_end = find_section_end(lines, header_idx);
    let mut out: Vec<String> = lines[..=header_idx].iter().map(|s| (*s).to_owned()).collect();
    let (mut has_command, mut has_args, mut changed) = (false, false, false);

    let mut i = header_idx + 1;
    while i < section_end {
        let line = lines[i];
        match toml_key_name(line) {
            Some("command") => {
                has_command = true;
                changed |= line.trim() != command_line;
                out.push(if line.trim() == command_line {
                    line.to_owned()
                } else {
                    command_line.to_owned()
                });
                i += 1;
            }
            Some("args") => {
                has_args = true;
                let span_end = find_value_end(lines, i, section_end);
                let is_exact = span_end == i + 1 && line.trim() == args_line;
                changed |= !is_exact;
                out.push(if is_exact { line.to_owned() } else { args_line.to_owned() });
                i = span_end;
            }
            _ => {
                out.push(line.to_owned());
                i += 1;
            }
        }
    }

    changed |=
        insert_missing_fields(&mut out, header_idx, command_line, args_line, has_command, has_args);
    out.extend(lines[section_end..].iter().map(|s| (*s).to_owned()));
    if !changed {
        return original.to_owned();
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

fn find_section_end(lines: &[&str], header_idx: usize) -> usize {
    let mut depth = 0i32;
    for (idx, line) in lines.iter().enumerate().skip(header_idx + 1) {
        let code = strip_toml_comment(line);
        if depth == 0 && is_table_header(code) {
            return idx;
        }
        depth = (depth + bracket_delta(code)).max(0);
    }
    lines.len()
}

fn find_value_end(lines: &[&str], start_idx: usize, section_end: usize) -> usize {
    let mut depth = bracket_delta(strip_toml_comment(lines[start_idx]));
    let mut idx = start_idx + 1;
    while depth > 0 && idx < section_end {
        depth += bracket_delta(strip_toml_comment(lines[idx]));
        idx += 1;
    }
    idx
}

fn bracket_delta(code: &str) -> i32 {
    let (mut delta, mut in_double, mut in_single, mut escaped) = (0i32, false, false, false);
    for ch in code.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_double => escaped = true,
            '"' if !in_single => in_double = !in_double,
            '\'' if !in_double => in_single = !in_single,
            '[' if !in_double && !in_single => delta += 1,
            ']' if !in_double && !in_single => delta -= 1,
            _ => {}
        }
    }
    delta
}

fn insert_missing_fields(
    lines: &mut Vec<String>,
    header_idx: usize,
    command_line: &str,
    args_line: &str,
    has_command: bool,
    has_args: bool,
) -> bool {
    let mut pos = header_idx + 1;
    let mut inserted = false;
    if !has_command {
        lines.insert(pos, command_line.to_owned());
        pos += 1;
        inserted = true;
    }
    if !has_args {
        lines.insert(pos, args_line.to_owned());
        inserted = true;
    }
    inserted
}

fn is_named_header(line: &str, header: &str) -> bool {
    strip_toml_comment(line).trim() == header
}

fn is_table_header(code: &str) -> bool {
    let trimmed = code.trim();
    if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return false;
    }
    let inner = trimmed.trim_start_matches('[').trim();
    !inner.starts_with(['"', '\'', '['])
}

fn toml_key_name(line: &str) -> Option<&str> {
    let code = strip_toml_comment(line);
    let (key, _) = code.split_once('=')?;
    let trimmed = key.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn strip_toml_comment(line: &str) -> &str {
    let (mut in_double, mut in_single, mut escaped) = (false, false, false);
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_double => escaped = true,
            '"' if !in_single => in_double = !in_double,
            '\'' if !in_double => in_single = !in_single,
            '#' if !in_double && !in_single => return &line[..idx],
            _ => {}
        }
    }
    line
}
