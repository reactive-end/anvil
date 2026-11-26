//! MCP tool handlers for `anvil-memory`: save, search, timeline, get, and context.

use anvil_memory::{MemoryKind, ObservationInput};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::McpServer;

/// Dispatches a `mem_*` MCP tool call against the repository's `MemoryEngine`.
pub fn call_memory_tool(server: &mut McpServer, name: &str, args: &Value) -> Result<Value> {
    match name {
        "mem_save" => handle_save(server, args),
        "mem_search" => handle_search(server, args),
        "mem_timeline" => handle_timeline(server, args),
        "mem_get" => handle_get(server, args),
        "mem_context" => handle_context(server, args),
        _ => bail!("unknown memory tool: {name}"),
    }
}

fn handle_save(server: &mut McpServer, args: &Value) -> Result<Value> {
    let kind_raw = required_str(args, "kind")?;
    let kind: MemoryKind = kind_raw.parse()?;
    let related_files = string_array_arg(args, "related_files");
    let mut related_symbols = string_array_arg(args, "related_symbols");
    if related_symbols.is_empty()
        && !related_files.is_empty()
        && let Ok(engine) = server.ensure_graph()
    {
        related_symbols = engine.exported_symbols_for_files(&related_files, 8);
    }
    let input = ObservationInput {
        topic_key: optional_str(args, "topic_key").map(str::to_owned),
        kind,
        title: required_str(args, "title")?.to_owned(),
        what: required_str(args, "what")?.to_owned(),
        why: required_str(args, "why")?.to_owned(),
        location: required_str(args, "where")
            .or_else(|_| required_str(args, "location"))?
            .to_owned(),
        learned: required_str(args, "learned")?.to_owned(),
        related_files,
        related_symbols,
        ttl_seconds: args.get("ttl_seconds").and_then(Value::as_i64),
    };

    let cache_was_clean = server.chunks_cache_is_clean();
    let mem = server.ensure_memory_no_sync()?;
    let saved = mem.save(input)?;
    if cache_was_clean {
        server.refresh_chunks_mtime();
    }
    Ok(json!({
        "saved": true,
        "observation": saved,
    }))
}

fn handle_search(server: &mut McpServer, args: &Value) -> Result<Value> {
    let query = args.get("query").and_then(Value::as_str).unwrap_or("");
    let kind = match optional_str(args, "kind") {
        Some(raw) => Some(raw.parse::<MemoryKind>()?),
        None => None,
    };
    let limit = args.get("limit").and_then(Value::as_u64).map_or(10, |l| l.clamp(1, 100) as usize);

    let mem = server.ensure_memory()?;
    let hits = mem.search(query, kind, limit)?;
    Ok(json!({
        "count": hits.len(),
        "hits": hits,
    }))
}

fn handle_timeline(server: &mut McpServer, args: &Value) -> Result<Value> {
    let id_or_topic = required_str(args, "id_or_topic")?;
    let before = args.get("before").and_then(Value::as_u64).map_or(5, |n| n.min(50) as usize);
    let after = args.get("after").and_then(Value::as_u64).map_or(5, |n| n.min(50) as usize);

    let mem = server.ensure_memory()?;
    let entries = mem.timeline(id_or_topic, before, after)?;
    Ok(json!({
        "count": entries.len(),
        "entries": entries,
    }))
}

fn handle_get(server: &mut McpServer, args: &Value) -> Result<Value> {
    let id = required_str(args, "id")?;
    let mem = server.ensure_memory()?;
    let obs = match mem.get(id)? {
        Some(found) => Some(found),
        None => mem.get_by_topic(id)?,
    };
    match obs {
        Some(observation) => Ok(json!({ "found": true, "observation": observation })),
        None => Ok(json!({ "found": false, "id": id })),
    }
}

fn handle_context(server: &mut McpServer, args: &Value) -> Result<Value> {
    let max_tokens =
        args.get("max_tokens").and_then(Value::as_u64).map_or(800, |t| t.min(8000) as usize);
    let mem = server.ensure_memory()?;
    let hits = mem.session_context(max_tokens)?;
    Ok(json!({
        "count": hits.len(),
        "max_tokens": max_tokens,
        "hits": hits,
    }))
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .with_context(|| format!("missing or empty required string argument `{key}`"))
}

fn optional_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn string_array_arg(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
