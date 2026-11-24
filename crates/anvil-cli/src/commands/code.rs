//! `anvil code exec`: local CLI runner for sandboxed JavaScript Code Mode scripts.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use anvil_mcp::McpServer;
use anvil_mcp::tools_codemode::call_code_exec;
use serde_json::json;

use crate::cli::subcmds::CodeSubcmd;

/// Executes `anvil code <exec>`.
pub fn execute(root: &Path, sub: CodeSubcmd) -> Result<()> {
    match sub {
        CodeSubcmd::Exec { script, max_calls } => {
            let source = if let Some(file) = script.strip_prefix('@') {
                fs::read_to_string(root.join(file))
                    .with_context(|| format!("failed to read script file `{file}`"))?
            } else {
                script
            };
            let mut server = McpServer::new(root)?;
            let out = call_code_exec(
                &mut server,
                &json!({ "script": source, "max_calls": max_calls }),
            )?;
            println!("{}", serde_json::to_string_pretty(&out)?);
            Ok(())
        }
    }
}
