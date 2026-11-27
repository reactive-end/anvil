//! `anvil db`: Google MCP Toolbox for Databases as the way to look at the application database,
//! for API scenarios (`anvil qa api`) and, as an MCP server, for agents.

mod check;
mod env_path;
mod init;

use std::path::Path;

use anvil_adapters::db_mcp;
use anvil_core::config::AnvilConfig;
use anvil_qa::api::db::{ToolboxClient, select};
use anyhow::Result;

use crate::cli::subcmds::DbSubcmd;
use crate::ui;

/// Executes `anvil db <subcommand>`.
pub fn execute(root: &Path, sub: DbSubcmd) -> Result<()> {
    match sub {
        DbSubcmd::Init { kind, database, name, toolbox, force } => init::run(
            root,
            &init::InitArgs {
                kind: &kind,
                database: database.as_deref(),
                name: &name,
                toolbox: toolbox.as_deref(),
                force,
            },
        ),
        DbSubcmd::Check => check::run(root),
        DbSubcmd::Query { sql } => query(root, &sql),
        DbSubcmd::Mcp { state } => set_mcp(root, state == "on"),
    }
}

fn query(root: &Path, sql: &str) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    let rows = select(&ToolboxClient::new(root, &config.db), sql)?;
    println!("{}", serde_json::to_string_pretty(&rows)?);
    Ok(())
}

fn set_mcp(root: &Path, on: bool) -> Result<()> {
    let mut config = AnvilConfig::load_or_default(root)?;
    config.db.mcp = on;
    config.save(root)?;
    let written = db_mcp::apply(root, config.adapters)?;
    if on {
        println!(
            "`{}` MCP server registered ({} file(s) updated)",
            db_mcp::DB_SERVER,
            written.len()
        );
        if !config.db.config_path(root).is_file() {
            println!(
                "  {} {} does not exist yet; run `anvil db init --kind <sqlite|postgres|mysql>`",
                ui::yellow("note:"),
                config.db.config
            );
        }
    } else {
        println!("`{}` MCP server removed ({} file(s) updated)", db_mcp::DB_SERVER, written.len());
    }
    for path in &written {
        println!("  updated {}", path.display());
    }
    Ok(())
}
