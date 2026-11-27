//! `anvil search`: the lookup cascade code graph -> docs -> code, answered by the first useful stage.

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_mcp::search_cascade::{CascadeOptions, CodeStage, DOCS_LIMIT, cascade, render};
use anyhow::{Result, bail};

use crate::cli::SearchArgs;

/// Executes `anvil search`: prints the rendered cascade (or the JSON report with `--json`).
pub fn execute(root: &Path, args: &SearchArgs) -> Result<()> {
    if args.query.trim().is_empty() {
        bail!("the query is empty");
    }
    let config = AnvilConfig::load_or_default(root)?;
    let opts = CascadeOptions {
        code: if args.code { CodeStage::Force } else { CodeStage::Auto },
        max_tokens: args.max_tokens.max(100),
        graph_limit: args.limit.max(1),
        docs_limit: args.limit.clamp(1, DOCS_LIMIT),
    };
    let report = cascade(root, &config, &args.query, &opts);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}", render(&report));
    }
    Ok(())
}
