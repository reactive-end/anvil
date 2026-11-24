//! Non-destructive adapter generator for `.codex/config.toml`.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::sync::{read_optional_string, write_if_changed};

const ANVIL_TOML_HEADER: &str = "[mcp_servers.anvil]";
const ANVIL_COMMAND_LINE: &str = r#"command = "anvil""#;
const ANVIL_ARGS_LINE: &str = r#"args = ["mcp"]"#;
const ANVIL_TOML_BLOCK: &str = "[mcp_servers.anvil]\ncommand = \"anvil\"\nargs = [\"mcp\"]\n";

/// Returns the path to `.codex/config.toml`.
pub fn config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".codex").join("config.toml")
}

/// Non-destructively ensures `[mcp_servers.anvil]` exists in `.codex/config.toml`.
pub fn merge_config_toml(repo_root: &Path) -> Result<bool> {
    let path = config_path(repo_root);
    let existing = read_optional_string(&path)?;
    let updated = merge_toml_content(existing.as_deref());
    write_if_changed(&path, &updated)
}

fn merge_toml_content(existing: Option<&str>) -> String {
    let Some(current) = existing else {
        return ANVIL_TOML_BLOCK.to_owned();
    };
    if current.trim().is_empty() {
        return ANVIL_TOML_BLOCK.to_owned();
    }

    let lines: Vec<&str> = current.lines().collect();
    if let Some(header_idx) = lines.iter().position(|line| is_anvil_header(line)) {
        return update_existing_section(current, &lines, header_idx);
    }

    append_anvil_block(current)
}

fn append_anvil_block(current: &str) -> String {
    let sep = if current.ends_with("\n\n") || current.ends_with("\r\n\r\n") {
        ""
    } else if current.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{current}{sep}{ANVIL_TOML_BLOCK}")
}

fn update_existing_section(original: &str, lines: &[&str], header_idx: usize) -> String {
    let section_end = (header_idx + 1..lines.len())
        .find(|&idx| is_table_header(lines[idx]))
        .unwrap_or(lines.len());

    let mut out: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    let mut has_command = false;
    let mut has_args = false;
    let mut changed = false;

    for item in out.iter_mut().take(section_end).skip(header_idx + 1) {
        match toml_key_name(item) {
            Some("command") => {
                has_command = true;
                changed |= set_line_if_different(item, ANVIL_COMMAND_LINE);
            }
            Some("args") => {
                has_args = true;
                changed |= set_line_if_different(item, ANVIL_ARGS_LINE);
            }
            _ => {}
        }
    }

    let insert_pos = insert_missing_fields(&mut out, header_idx, has_command, has_args);
    changed |= insert_pos;

    if !changed {
        return original.to_owned();
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

fn set_line_if_different(slot: &mut String, expected: &str) -> bool {
    if slot.trim() == expected {
        return false;
    }
    *slot = expected.to_owned();
    true
}

fn insert_missing_fields(
    lines: &mut Vec<String>,
    header_idx: usize,
    has_command: bool,
    has_args: bool,
) -> bool {
    let mut pos = header_idx + 1;
    let mut inserted = false;
    if !has_command {
        lines.insert(pos, ANVIL_COMMAND_LINE.to_owned());
        pos += 1;
        inserted = true;
    }
    if !has_args {
        lines.insert(pos, ANVIL_ARGS_LINE.to_owned());
        inserted = true;
    }
    inserted
}

fn is_anvil_header(line: &str) -> bool {
    strip_toml_comment(line).trim() == ANVIL_TOML_HEADER
}

fn is_table_header(line: &str) -> bool {
    let trimmed = strip_toml_comment(line).trim();
    trimmed.starts_with('[') && trimmed.ends_with(']')
}

fn toml_key_name(line: &str) -> Option<&str> {
    let code = strip_toml_comment(line);
    let (key, _) = code.split_once('=')?;
    let trimmed = key.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_double = false;
    let mut in_single = false;
    let mut escaped = false;
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
