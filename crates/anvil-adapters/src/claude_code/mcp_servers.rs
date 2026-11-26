//! The MCP servers Claude Code would start in a repository, read from its own config files.
//!
//! Servers the desktop app adds on its own (browser, computer use, and similar) live in no file;
//! callers accept those by name.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// An MCP server and where it was configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    /// The name Claude Code puts between `mcp__` and `__` in the server's tool names.
    pub name: String,
    /// `project`, `user`, `local`, or `plugin <name>`.
    pub source: String,
}

impl McpServer {
    /// The allowlist entry that grants every tool of this server.
    pub fn tool_pattern(&self) -> String {
        tool_pattern(&self.name)
    }
}

/// The allowlist entry that grants every tool of the server called `name`.
pub fn tool_pattern(name: &str) -> String {
    format!("mcp__{name}__*")
}

/// Where Claude Code keeps its per-user files.
#[derive(Debug, Clone)]
pub struct ClaudeHome {
    /// `~/.claude` (or `CLAUDE_CONFIG_DIR`): settings and plugins.
    pub dir: PathBuf,
    /// `~/.claude.json`: user- and local-scope MCP servers.
    pub state: PathBuf,
}

impl ClaudeHome {
    /// Finds the current user's Claude Code files, honoring `CLAUDE_CONFIG_DIR`.
    pub fn locate() -> Option<Self> {
        if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from) {
            let state = dir.join(".claude.json");
            return Some(Self { dir, state });
        }
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
        let home = PathBuf::from(home);
        Some(Self { dir: home.join(".claude"), state: home.join(".claude.json") })
    }
}

/// Lists the configured MCP servers once each, in project, local, user, then plugin order.
///
/// Unreadable or malformed files contribute nothing, since every source is optional.
pub fn discover(repo_root: &Path, home: &ClaudeHome) -> Vec<McpServer> {
    let mut found = Vec::new();
    let mut add = |names: Vec<String>, source: &str| {
        for name in names {
            if !found.iter().any(|s: &McpServer| s.name == name) {
                found.push(McpServer { name, source: source.to_owned() });
            }
        }
    };
    add(server_names(&read_object(&repo_root.join(".mcp.json"))), "project");
    let state = read_object(&home.state);
    let local = state.get("projects").and_then(Value::as_object).and_then(|projects| {
        projects
            .iter()
            .find(|(path, _)| same_path(path, repo_root))
            .and_then(|(_, project)| project.as_object())
    });
    add(local.map(server_names).unwrap_or_default(), "local");
    add(server_names(&state), "user");
    for (plugin, dir) in enabled_plugins(repo_root, home) {
        let names = plugin_servers(&dir)
            .into_iter()
            .map(|server| format!("plugin_{plugin}_{server}"))
            .collect();
        add(names, &format!("plugin {plugin}"));
    }
    found
}

fn server_names(config: &Map<String, Value>) -> Vec<String> {
    config
        .get("mcpServers")
        .and_then(Value::as_object)
        .map(|servers| servers.keys().map(|name| tool_name_part(name)).collect())
        .unwrap_or_default()
}

/// Claude Code keeps letters, digits, `_`, and `-` in tool names and turns the rest into `_`.
fn tool_name_part(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

/// Enabled plugins and their install folders; project settings override user settings.
fn enabled_plugins(repo_root: &Path, home: &ClaudeHome) -> Vec<(String, PathBuf)> {
    let mut enabled = Map::new();
    let settings = [
        home.dir.join("settings.json"),
        repo_root.join(".claude").join("settings.json"),
        repo_root.join(".claude").join("settings.local.json"),
    ];
    for path in settings {
        if let Some(Value::Object(flags)) = read_object(&path).remove("enabledPlugins") {
            enabled.extend(flags);
        }
    }
    let installed = read_object(&home.dir.join("plugins").join("installed_plugins.json"));
    let Some(plugins) = installed.get("plugins").and_then(Value::as_object) else {
        return Vec::new();
    };
    enabled
        .iter()
        .filter(|(_, on)| on.as_bool() == Some(true))
        .filter_map(|(id, _)| {
            let entry = plugins.get(id)?;
            let entry = entry.as_array().map_or(Some(entry), |all| all.first())?;
            let dir = entry.get("installPath")?.as_str()?;
            let name = id.split('@').next().unwrap_or(id);
            Some((tool_name_part(name), PathBuf::from(dir)))
        })
        .collect()
}

/// Servers a plugin declares in `.mcp.json` (wrapped in `mcpServers` or not) or `plugin.json`.
fn plugin_servers(dir: &Path) -> Vec<String> {
    let mcp = read_object(&dir.join(".mcp.json"));
    if mcp.contains_key("mcpServers") {
        return server_names(&mcp);
    }
    if !mcp.is_empty() {
        return mcp.keys().map(|name| tool_name_part(name)).collect();
    }
    server_names(&read_object(&dir.join(".claude-plugin").join("plugin.json")))
}

fn read_object(path: &Path) -> Map<String, Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
}

/// Compares a path key from `~/.claude.json` with the repository root, ignoring separators,
/// the Windows verbatim prefix, and (on Windows) letter case.
fn same_path(key: &str, root: &Path) -> bool {
    let clean = |raw: &str| {
        let unified = raw.trim_start_matches(r"\\?\").replace('\\', "/");
        let trimmed = unified.trim_end_matches('/').to_owned();
        if cfg!(windows) { trimmed.to_lowercase() } else { trimmed }
    };
    clean(key) == clean(&root.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, json: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, json).expect("write");
    }

    #[test]
    fn reads_every_source_once_with_tool_names() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        let home =
            ClaudeHome { dir: tmp.path().join(".claude"), state: tmp.path().join(".claude.json") };
        let plugin_dir = tmp.path().join("plugins").join("monet");
        write(&repo.join(".mcp.json"), r#"{"mcpServers":{"anvil":{},"postgres":{}}}"#);
        let repo_key = repo.to_string_lossy().replace('\\', "/");
        write(
            &home.state,
            &serde_json::json!({
                "mcpServers": {"jira": {}, "postgres": {}},
                "projects": {repo_key: {"mcpServers": {"chrome devtools": {}}}},
            })
            .to_string(),
        );
        write(
            &home.dir.join("settings.json"),
            r#"{"enabledPlugins":{"monet@monet":true,"off@x":false}}"#,
        );
        write(
            &home.dir.join("plugins").join("installed_plugins.json"),
            &serde_json::json!({"plugins": {
                "monet@monet": [{"installPath": plugin_dir.to_string_lossy()}],
                "off@x": [{"installPath": plugin_dir.to_string_lossy()}],
            }})
            .to_string(),
        );
        write(&plugin_dir.join(".mcp.json"), r#"{"chrome-devtools":{}}"#);

        let found: Vec<(String, String)> =
            discover(&repo, &home).into_iter().map(|s| (s.name, s.source)).collect();
        let expected = [
            ("anvil", "project"),
            ("postgres", "project"),
            ("chrome_devtools", "local"),
            ("jira", "user"),
            ("plugin_monet_chrome-devtools", "plugin monet"),
        ];
        let expected: Vec<(String, String)> =
            expected.iter().map(|(n, s)| ((*n).to_owned(), (*s).to_owned())).collect();
        assert_eq!(found, expected);
    }

    #[test]
    fn missing_files_find_nothing() {
        let tmp = tempfile::tempdir().expect("tmp");
        let home = ClaudeHome { dir: tmp.path().join("none"), state: tmp.path().join("none.json") };
        assert!(discover(tmp.path(), &home).is_empty());
    }
}
