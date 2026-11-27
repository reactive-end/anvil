//! The Chrome DevTools server entry in `.codex/config.toml`.

use std::path::Path;

use anyhow::Result;

use super::{CHROME_DEVTOOLS_SERVER, is_managed_args, launch_command};
use crate::codex::{find_section_end, is_named_header, upsert_toml_section};
use crate::sync::{read_optional_string, write_if_changed};

fn header() -> String {
    format!("[mcp_servers.{CHROME_DEVTOOLS_SERVER}]")
}

/// Adds or removes the managed `[mcp_servers.chrome-devtools]` section; a section with other
/// arguments belongs to the user and stays. Returns whether the file changed.
pub(super) fn merge_codex_server(path: &Path, enabled: bool, windows: bool) -> Result<bool> {
    let existing = read_optional_string(path)?;
    if !enabled {
        let Some(current) = existing else { return Ok(false) };
        return match remove_managed_section(&current) {
            Some(updated) => write_if_changed(path, &updated),
            None => Ok(false),
        };
    }
    if existing.as_deref().is_some_and(|text| section_text(text).is_some_and(|s| !is_managed(&s))) {
        return Ok(false);
    }
    let (command, args) = launch_command(windows);
    let command_line = format!("command = {}", serde_json::to_string(&command)?);
    let args_line = format!("args = {}", serde_json::to_string(&args)?);
    let updated = upsert_toml_section(existing.as_deref(), &header(), &command_line, &args_line);
    write_if_changed(path, &updated)
}

/// `true` when the file defines the server, managed or not.
pub(super) fn codex_server_present(path: &Path) -> bool {
    read_optional_string(path).ok().flatten().is_some_and(|text| section_text(&text).is_some())
}

fn section_bounds(lines: &[&str]) -> Option<(usize, usize)> {
    let head = header();
    let start = lines.iter().position(|line| is_named_header(line, &head))?;
    Some((start, find_section_end(lines, start)))
}

fn section_text(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let (start, end) = section_bounds(&lines)?;
    Some(lines[start..end].join("\n"))
}

fn is_managed(section: &str) -> bool {
    is_managed_args(section.split('"').skip(1).step_by(2))
}

fn remove_managed_section(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let (mut start, end) = section_bounds(&lines)?;
    if !is_managed(&lines[start..end].join("\n")) {
        return None;
    }
    if start > 0 && lines[start - 1].trim().is_empty() {
        start -= 1;
    }
    let kept: Vec<&str> = lines[..start].iter().chain(&lines[end..]).copied().collect();
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_is_added_updated_and_removed_without_touching_other_servers() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "model = \"x\"\n\n[mcp_servers.anvil]\ncommand = \"anvil\"\nargs = [\"mcp\"]\n",
        )
        .expect("seed");

        assert!(merge_codex_server(&path, true, false).expect("add"));
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("[mcp_servers.chrome-devtools]"));
        assert!(text.contains("--no-performance-crux"));
        assert!(text.contains("[mcp_servers.anvil]"));
        assert!(!merge_codex_server(&path, true, false).expect("idempotent"));

        assert!(merge_codex_server(&path, false, false).expect("remove"));
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(!text.contains("chrome-devtools"));
        assert!(text.contains("model = \"x\""));
        assert!(text.contains("[mcp_servers.anvil]"));
        assert!(!merge_codex_server(&path, false, false).expect("nothing left"));
    }

    #[test]
    fn a_user_section_with_other_arguments_is_left_alone() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("config.toml");
        let user = "[mcp_servers.chrome-devtools]\ncommand = \"npx\"\nargs = [\"chrome-devtools-mcp@latest\"]\n";
        std::fs::write(&path, user).expect("seed");
        assert!(!merge_codex_server(&path, true, false).expect("keep on enable"));
        assert!(!merge_codex_server(&path, false, false).expect("keep on disable"));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), user);
        assert!(codex_server_present(&path));
    }
}
