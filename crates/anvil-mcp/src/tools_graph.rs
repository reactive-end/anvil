//! MCP tool handlers for `anvil-graph`: impact, origin, query, path, explain, read_symbol, outline.

use anyhow::{Context, Result, bail};
use anvil_core::git::{self, DiffScope};
use anvil_core::stats::{StatsEvent, record_event};
use serde_json::{Value, json};

use crate::McpServer;

/// Dispatches a `graph_*` MCP tool call using the server's cached/fresh code graph.
pub fn call_graph_tool(server: &mut McpServer, name: &str, args: &Value) -> Result<Value> {
    match name {
        "graph_impact" => handle_impact(server, args),
        "graph_origin" => handle_origin(server, args),
        "graph_query" => handle_query(server, args),
        "graph_path" => handle_path(server, args),
        "graph_explain" => handle_explain(server, args),
        "graph_read_symbol" => handle_read_symbol(server, args),
        "graph_outline" => handle_outline(server, args),
        _ => bail!("unknown graph tool: {name}"),
    }
}

fn handle_impact(server: &mut McpServer, args: &Value) -> Result<Value> {
    let mut files = string_array_arg(args, "files");
    if files.is_empty() {
        files = git::changed_files(server.repo_root(), DiffScope::WorkingTree).unwrap_or_default();
    }
    let depth = args
        .get("depth")
        .and_then(Value::as_u64)
        .map_or(2, |d| d as usize);
    let run_typecheck = args
        .get("typecheck")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let engine = server.ensure_graph()?;
    let report = engine.blast_radius(&files, depth);

    let typecheck_outputs = if run_typecheck {
        let mut all_files = report.changed_files.clone();
        for item in &report.impacted_files {
            if !all_files.contains(&item.file) {
                all_files.push(item.file.clone());
            }
        }
        Some(engine.run_impacted_typecheck(&all_files)?)
    } else {
        None
    };

    Ok(json!({
        "blast_radius": report,
        "typecheck": typecheck_outputs,
    }))
}

fn handle_origin(server: &mut McpServer, args: &Value) -> Result<Value> {
    let target = required_str(args, "target")?;
    let engine = server.ensure_graph()?;
    match engine.find_origin(target) {
        Some(origin) => Ok(json!({ "found": true, "origin": origin })),
        None => Ok(json!({ "found": false, "target": target })),
    }
}

fn handle_query(server: &mut McpServer, args: &Value) -> Result<Value> {
    let term = required_str(args, "term")?;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(10, |l| l.clamp(1, 100) as usize);
    let engine = server.ensure_graph()?;
    let hits = engine.query(term, limit);
    Ok(json!({
        "count": hits.len(),
        "hits": hits,
    }))
}

fn handle_path(server: &mut McpServer, args: &Value) -> Result<Value> {
    let from = required_str(args, "from")?;
    let to = required_str(args, "to")?;
    let engine = server.ensure_graph()?;
    match engine.shortest_path(from, to) {
        Some(path) => Ok(json!({ "found": true, "path": path })),
        None => Ok(json!({ "found": false, "from": from, "to": to })),
    }
}

fn handle_explain(server: &mut McpServer, args: &Value) -> Result<Value> {
    let target = required_str(args, "target")?;
    let engine = server.ensure_graph()?;
    match engine.explain(target) {
        Some(explain) => Ok(json!({ "found": true, "explain": explain })),
        None => Ok(json!({ "found": false, "target": target })),
    }
}

fn handle_read_symbol(server: &mut McpServer, args: &Value) -> Result<Value> {
    let symbol = required_str(args, "symbol")?;
    let include_imports = args
        .get("include_imports")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let repo_root = server.repo_root().to_path_buf();
    let engine = server.ensure_graph()?;
    match engine.read_symbol(symbol, include_imports)? {
        Some(slice) => {
            if slice.estimated_tokens_saved > 0 {
                let _ = record_event(
                    &repo_root,
                    StatsEvent::SymbolReadSaved {
                        tokens_saved: slice.estimated_tokens_saved,
                    },
                );
            }
            Ok(json!({ "found": true, "slice": slice }))
        }
        None => Ok(json!({ "found": false, "symbol": symbol })),
    }
}

fn handle_outline(server: &mut McpServer, args: &Value) -> Result<Value> {
    let target = args
        .get("target")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let max_tokens = args
        .get("max_tokens")
        .and_then(Value::as_u64)
        .map_or(1500, |m| m.clamp(1, 16_000) as usize);
    let repo_root = server.repo_root().to_path_buf();
    let engine = server.ensure_graph()?;
    let outline = engine.outline(target, max_tokens);
    if outline.estimated_tokens_saved > 0 {
        let _ = record_event(
            &repo_root,
            StatsEvent::OutlineSaved {
                tokens_saved: outline.estimated_tokens_saved,
            },
        );
    }
    Ok(json!({ "outline": outline }))
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .with_context(|| format!("missing or empty required string argument `{key}`"))
}

fn string_array_arg(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(CodeGraphEngineNormalize::norm)
                .collect()
        })
        .unwrap_or_default()
}

struct CodeGraphEngineNormalize;
impl CodeGraphEngineNormalize {
    fn norm(raw: &str) -> String {
        anvil_graph::model::CodeGraph::normalize_rel_path(raw)
    }
}
