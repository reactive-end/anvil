//! `anvil orchestrator tools`: choose which MCP servers executors load.

use std::path::Path;

use anyhow::{Result, bail};
use anvil_adapters::claude_code::agents;
use anvil_adapters::claude_code::mcp_servers::{self, ClaudeHome, McpServer};
use anvil_core::config::AnvilConfig;
use anvil_core::config::orchestrator::{OrchestratorConfig, is_tool_name};

use crate::commands::ExitCodeError;
use crate::ui::prompt::{self, PromptOutcome, SelectOption};

/// The flags of `anvil orchestrator tools`.
pub struct ToolsArgs {
    pub add: Vec<String>,
    pub remove: Vec<String>,
    pub list: bool,
}

/// Lists, picks, or edits the MCP servers in `orchestrator.executor_tools`, then re-syncs the agents.
pub fn execute(root: &Path, args: &ToolsArgs) -> Result<()> {
    let editing = !args.add.is_empty() || !args.remove.is_empty();
    let servers = discovered(root);
    if args.list || (!editing && !prompt::is_interactive_tui()) {
        let current = AnvilConfig::load_or_default(root)?.orchestrator.unwrap_or_default();
        print_list(&servers, &current.executor_tools);
        return Ok(());
    }
    let mut config = AnvilConfig::load(root)?;
    let current = config.orchestrator.clone().unwrap_or_default();
    let chosen = if editing {
        edit(&current.executor_tools, &args.add, &args.remove)?
    } else {
        pick(&servers, &current.executor_tools)?
    };
    if chosen != current.executor_tools {
        let next = OrchestratorConfig {
            executor_tools: chosen,
            ..current
        };
        config.orchestrator = Some(next.clone());
        config.save(root)?;
        for path in agents::sync_agents(root, &next)? {
            println!("wrote {}", path.display());
        }
        print_list(&servers, &next.executor_tools);
        println!("Executors started from now on load this list; a running Claude Code session may keep the old one until it restarts.");
    } else {
        print_list(&servers, &current.executor_tools);
    }
    Ok(())
}

/// Configured servers except Anvil's own, which executors always load.
fn discovered(root: &Path) -> Vec<McpServer> {
    ClaudeHome::locate()
        .map(|home| mcp_servers::discover(root, &home))
        .unwrap_or_default()
        .into_iter()
        .filter(|server| server.name != "anvil")
        .collect()
}

/// `postgres` becomes `mcp__postgres__*`; a full `mcp__…` entry is kept as written.
fn entry(raw: &str) -> Result<String> {
    let raw = raw.trim();
    let entry = if raw.starts_with("mcp__") {
        raw.to_owned()
    } else {
        mcp_servers::tool_pattern(raw)
    };
    if !is_tool_name(&entry) {
        bail!("`{raw}` is not an MCP server name or an `mcp__<server>__*` entry");
    }
    Ok(entry)
}

fn edit(current: &[String], add: &[String], remove: &[String]) -> Result<Vec<String>> {
    let removed: Vec<String> = remove.iter().map(|raw| entry(raw)).collect::<Result<_>>()?;
    let mut tools: Vec<String> = current
        .iter()
        .filter(|tool| !removed.contains(tool))
        .cloned()
        .collect();
    for raw in add {
        let added = entry(raw)?;
        if !tools.contains(&added) {
            tools.push(added);
        }
    }
    Ok(tools)
}

/// A checklist of the discovered servers plus configured ones no file names any more.
fn pick(servers: &[McpServer], current: &[String]) -> Result<Vec<String>> {
    let mut rows: Vec<(String, String, String)> = servers
        .iter()
        .map(|s| (s.name.clone(), s.source.clone(), s.tool_pattern()))
        .collect();
    for tool in current {
        let server = tool.strip_prefix("mcp__").and_then(|rest| rest.strip_suffix("__*"));
        if let Some(name) = server.filter(|_| rows.iter().all(|(_, _, t)| t != tool)) {
            rows.push((name.to_owned(), "added by name".to_owned(), tool.clone()));
        }
    }
    if rows.is_empty() {
        println!("No MCP servers found in Claude Code's config; add one with `--add <name>`.");
        return Ok(current.to_vec());
    }
    let options: Vec<SelectOption> = rows
        .iter()
        .map(|(name, source, _)| SelectOption::new(name.as_str(), source.as_str()))
        .collect();
    let defaults: Vec<bool> = rows.iter().map(|(_, _, tool)| current.contains(tool)).collect();
    let subtitle = "Anvil's own tools are always included. Space toggles, Enter saves.";
    let selected = match prompt::multi_select(
        "MCP servers for executors",
        "Executor MCP servers",
        Some(subtitle),
        &options,
        &defaults,
    )? {
        PromptOutcome::Confirmed(selected) => selected,
        PromptOutcome::Cancelled => return Err(ExitCodeError::silent(130).into()),
    };
    let offered: Vec<&String> = rows.iter().map(|(_, _, tool)| tool).collect();
    let mut tools: Vec<String> = current
        .iter()
        .filter(|tool| !offered.contains(tool))
        .cloned()
        .collect();
    let picked = rows.iter().zip(selected).filter(|(_, on)| *on);
    tools.extend(picked.map(|((_, _, tool), _)| tool.clone()));
    Ok(tools)
}

fn print_list(servers: &[McpServer], tools: &[String]) {
    println!("MCP servers executors load (Anvil's own always included):");
    for server in servers {
        let mark = if tools.contains(&server.tool_pattern()) { "x" } else { " " };
        println!("  [{mark}] {:<32} {}", server.name, server.source);
    }
    let known: Vec<String> = servers.iter().map(McpServer::tool_pattern).collect();
    for tool in tools.iter().filter(|tool| !known.contains(tool)) {
        println!("  [x] {tool:<32} added by name");
    }
    println!("Servers the desktop app adds by itself are in no config file; add them with `--add <name>`.");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn edit_adds_names_once_and_removes_either_form() {
        let current = owned(&["mcp__jira__*", "Extra"]);
        let next = edit(
            &current,
            &owned(&["postgres", "mcp__postgres__*", "chrome-devtools"]),
            &owned(&["jira"]),
        )
        .expect("edit");
        assert_eq!(next, owned(&["Extra", "mcp__postgres__*", "mcp__chrome-devtools__*"]));
        let next = edit(&next, &[], &owned(&["mcp__postgres__*"])).expect("edit");
        assert_eq!(next, owned(&["Extra", "mcp__chrome-devtools__*"]));
    }

    #[test]
    fn edit_refuses_names_that_could_break_the_frontmatter() {
        assert!(edit(&[], &owned(&["pg\nmodel: opus"]), &[]).is_err());
        assert!(edit(&[], &owned(&["a b"]), &[]).is_err());
    }
}
