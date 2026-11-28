//! A managed server's section in `.codex/config.toml`.

use std::path::Path;

use anyhow::Result;

use super::ManagedServer;
use crate::codex::{find_section_end, is_named_header, upsert_toml_section};
use crate::sync::{read_optional_string, write_if_changed};

fn header(name: &str) -> String {
    format!("[mcp_servers.{name}]")
}

/// Adds or removes the managed `[mcp_servers.<name>]` section; a section with other arguments
/// belongs to the user and stays. Returns whether the file changed.
pub(super) fn merge_codex_server(
    path: &Path,
    server: &ManagedServer,
    enabled: bool,
) -> Result<bool> {
    let existing = read_optional_string(path)?;
    if !enabled {
        let Some(current) = existing else { return Ok(false) };
        return match remove_managed_section(&current, server) {
            Some(updated) => write_if_changed(path, &updated),
            None => Ok(false),
        };
    }
    let foreign = existing.as_deref().is_some_and(|text| {
        section_text(text, server.name).is_some_and(|section| !is_managed(&section, server))
    });
    if foreign {
        return Ok(false);
    }
    let command_line = format!("command = {}", serde_json::to_string(&server.command)?);
    let args_line = format!("args = {}", serde_json::to_string(&server.args)?);
    let updated =
        upsert_toml_section(existing.as_deref(), &header(server.name), &command_line, &args_line);
    write_if_changed(path, &updated)
}

/// `true` when the file defines the server called `name`, managed or not.
pub(super) fn codex_server_present(path: &Path, name: &str) -> bool {
    read_optional_string(path)
        .ok()
        .flatten()
        .is_some_and(|text| section_text(&text, name).is_some())
}

fn section_bounds(lines: &[&str], name: &str) -> Option<(usize, usize)> {
    let head = header(name);
    let start = lines.iter().position(|line| is_named_header(line, &head))?;
    Some((start, find_section_end(lines, start)))
}

fn section_text(text: &str, name: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let (start, end) = section_bounds(&lines, name)?;
    Some(lines[start..end].join("\n"))
}

fn is_managed(section: &str, server: &ManagedServer) -> bool {
    let quoted: Vec<&str> = section.split('"').skip(1).step_by(2).collect();
    (server.recognizes)(&quoted)
}

fn remove_managed_section(text: &str, server: &ManagedServer) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let (mut start, end) = section_bounds(&lines, server.name)?;
    if !is_managed(&lines[start..end].join("\n"), server) {
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
    use crate::web_qa;

    fn chrome() -> ManagedServer {
        web_qa::managed_server(false)
    }

    #[test]
    fn section_is_added_updated_and_removed_without_touching_other_servers() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "model = \"x\"\n\n[mcp_servers.anvil]\ncommand = \"anvil\"\nargs = [\"mcp\"]\n",
        )
        .expect("seed");

        assert!(merge_codex_server(&path, &chrome(), true).expect("add"));
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("[mcp_servers.chrome-devtools]"));
        assert!(text.contains("--no-performance-crux"));
        assert!(text.contains("[mcp_servers.anvil]"));
        assert!(!merge_codex_server(&path, &chrome(), true).expect("idempotent"));

        assert!(merge_codex_server(&path, &chrome(), false).expect("remove"));
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(!text.contains("chrome-devtools"));
        assert!(text.contains("model = \"x\""));
        assert!(text.contains("[mcp_servers.anvil]"));
        assert!(!merge_codex_server(&path, &chrome(), false).expect("nothing left"));
    }

    #[test]
    fn a_user_section_with_other_arguments_is_left_alone() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("config.toml");
        let user = "[mcp_servers.chrome-devtools]\ncommand = \"npx\"\nargs = [\"chrome-devtools-mcp@latest\"]\n";
        std::fs::write(&path, user).expect("seed");
        assert!(!merge_codex_server(&path, &chrome(), true).expect("keep on enable"));
        assert!(!merge_codex_server(&path, &chrome(), false).expect("keep on disable"));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), user);
        assert!(codex_server_present(&path, "chrome-devtools"));
    }
}
