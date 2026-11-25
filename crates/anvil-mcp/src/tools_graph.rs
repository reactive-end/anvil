//! MCP tool handlers for `anvil-graph`: impact, origin, query, path, explain, read_symbol, outline, context_pack.

use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use anvil_core::circuit_breaker::CircuitBreaker;
use anvil_core::config::AnvilConfig;
use anvil_core::git::{self, DiffScope};
use anvil_core::stats::{StatsEvent, estimate_tokens, record_event};
use anvil_graph::CodeGraphEngine;
use anvil_graph::query::{ExplainResult, FileOutline};
use anvil_memory::MemoryEngine;
use anvil_memory::model::SearchHit;
use serde::Serialize;
use serde_json::{Value, json};

use crate::McpServer;

/// Dispatches a `graph_*` or `context_pack` MCP tool call using the server's cached/fresh code graph.
pub fn call_graph_tool(server: &mut McpServer, name: &str, args: &Value) -> Result<Value> {
    match name {
        "graph_impact" => handle_impact(server, args),
        "graph_origin" => handle_origin(server, args),
        "graph_query" => handle_query(server, args),
        "graph_path" => handle_path(server, args),
        "graph_explain" => handle_explain(server, args),
        "graph_read_symbol" => handle_read_symbol(server, args),
        "graph_outline" => handle_outline(server, args),
        "context_pack" => handle_context_pack(server, args),
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

    if run_typecheck {
        let limit = AnvilConfig::load_or_default(server.repo_root())
            .map_or(3, |c| c.circuit_breaker_limit);
        if let Ok(st) = CircuitBreaker::new(server.repo_root(), limit).status()
            && st.is_open()
        {
            bail!(
                "Circuit Breaker is OPEN ({} identical failures); run `anvil circuit reset` before running typecheck.",
                st.consecutive_failures
            );
        }
    }

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

#[derive(Serialize)]
struct ContextPacket {
    targets: Vec<String>,
    resolved_files: Vec<String>,
    outlines: Vec<FileOutline>,
    edges: Vec<String>,
    memories: Vec<SearchHit>,
    related_tests: Vec<String>,
    estimated_tokens: u64,
}

struct GraphContext {
    resolved_files: Vec<String>,
    outlines: Vec<FileOutline>,
    edges: Vec<String>,
    related_tests: Vec<String>,
}

fn handle_context_pack(server: &mut McpServer, args: &Value) -> Result<Value> {
    let targets = string_array_arg(args, "targets");
    if targets.is_empty() {
        bail!("context_pack requires non-empty `targets` array");
    }
    let max_tokens = args
        .get("max_tokens")
        .and_then(Value::as_u64)
        .map_or(1200, |m| m.clamp(200, 8000) as usize);

    let graph_ctx = collect_graph_context(server.ensure_graph()?, &targets, max_tokens);
    let memories = collect_memory_context(server.ensure_memory()?, &targets)?;
    let mut packet = ContextPacket {
        targets,
        resolved_files: graph_ctx.resolved_files,
        outlines: graph_ctx.outlines,
        edges: graph_ctx.edges,
        memories,
        related_tests: graph_ctx.related_tests,
        estimated_tokens: 0,
    };
    fit_packet_to_budget(&mut packet, max_tokens)?;
    Ok(serde_json::to_value(&packet)?)
}

fn collect_graph_context(
    engine: &CodeGraphEngine,
    targets: &[String],
    max_tokens: usize,
) -> GraphContext {
    let mut resolved_files = Vec::new();
    let mut seen_files = HashSet::new();
    let mut edges = Vec::new();
    let mut seen_edges = HashSet::new();

    for target in targets {
        let explained = engine.explain(target);
        if engine.graph.file_hashes.contains_key(target) {
            push_unique(&mut resolved_files, &mut seen_files, target.clone());
        } else if let Some(ref exp) = explained {
            push_unique(&mut resolved_files, &mut seen_files, exp.node.file.clone());
        }
        if let Some(ref exp) = explained {
            push_explain_edges(exp, &mut edges, &mut seen_edges);
        }
    }

    let blast = engine.blast_radius(&resolved_files, 1);
    let outline_budget = (max_tokens / 3).max(150);
    let mut outlines = Vec::new();
    for file in resolved_files.iter().take(5) {
        outlines.extend(engine.outline(Some(file), outline_budget).files);
    }

    GraphContext {
        resolved_files,
        outlines,
        edges,
        related_tests: blast.related_tests,
    }
}

fn push_explain_edges(exp: &ExplainResult, edges: &mut Vec<String>, seen: &mut HashSet<String>) {
    for inc in exp.incoming.iter().take(4) {
        let entry = format!("<- {:?} {} ({}:{})", inc.kind, inc.node_id, inc.file, inc.line);
        push_unique(edges, seen, entry);
    }
    for out in exp.outgoing.iter().take(4) {
        let entry = format!("-> {:?} {} ({}:{})", out.kind, out.node_id, out.file, out.line);
        push_unique(edges, seen, entry);
    }
}

fn push_unique(items: &mut Vec<String>, seen: &mut HashSet<String>, value: String) {
    if seen.insert(value.clone()) {
        items.push(value);
    }
}

fn collect_memory_context(mem: &MemoryEngine, targets: &[String]) -> Result<Vec<SearchHit>> {
    let mut hits = Vec::new();
    let mut seen_ids = HashSet::new();
    for target in targets {
        for hit in mem.search(target, None, 2)? {
            if seen_ids.insert(hit.id.clone()) {
                hits.push(hit);
                if hits.len() >= 6 {
                    return Ok(hits);
                }
            }
        }
    }
    Ok(hits)
}

fn fit_packet_to_budget(packet: &mut ContextPacket, max_tokens: usize) -> Result<()> {
    loop {
        let tokens = estimate_tokens(serde_json::to_string(packet)?.len());
        if tokens as usize <= max_tokens {
            packet.estimated_tokens = tokens;
            return Ok(());
        }
        let trimmed = packet.edges.pop().is_some()
            || packet.memories.pop().is_some()
            || packet.related_tests.pop().is_some()
            || shrink_one_outline(&mut packet.outlines);
        if !trimmed {
            packet.estimated_tokens = tokens;
            return Ok(());
        }
    }
}

fn shrink_one_outline(outlines: &mut Vec<FileOutline>) -> bool {
    let Some(last) = outlines.last_mut() else {
        return false;
    };
    if last.symbols.len() > 1 {
        last.symbols.pop();
    } else {
        outlines.pop();
    }
    true
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
