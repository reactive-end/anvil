//! MCP lifecycle routing, tool schemas, and tool call dispatch.

use serde_json::{Value, json};

use crate::McpServer;
use crate::tools_graph::call_graph_tool;
use crate::tools_memory::call_memory_tool;
use crate::transport::{
    INTERNAL_ERROR, INVALID_PARAMS, JsonRpcRequest, JsonRpcResponse, METHOD_NOT_FOUND,
};

const DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";

/// Dispatches an incoming MCP JSON-RPC 2.0 request or notification.
pub fn dispatch(server: &mut McpServer, req: &JsonRpcRequest) -> Option<JsonRpcResponse> {
    let Some(id) = req.id.clone() else {
        if req.method == "notifications/initialized" || req.method == "initialized" {
            server.mark_initialized();
        }
        return None;
    };

    let resp = match req.method.as_str() {
        "initialize" => {
            server.mark_initialized();
            let requested = req
                .params
                .as_ref()
                .and_then(|p| p.get("protocolVersion"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            JsonRpcResponse::ok(id, initialize_result(requested))
        }
        "ping" => JsonRpcResponse::ok(id, json!({})),
        "tools/list" => JsonRpcResponse::ok(id, json!({ "tools": tool_definitions() })),
        "tools/call" => dispatch_tool_call(server, id, req.params.as_ref()),
        other => JsonRpcResponse::err(id, METHOD_NOT_FOUND, format!("unknown method: {other}")),
    };
    Some(resp)
}

fn initialize_result(requested_version: Option<&str>) -> Value {
    let negotiated = requested_version.unwrap_or(DEFAULT_PROTOCOL_VERSION);
    json!({
        "protocolVersion": negotiated,
        "capabilities": {
            "tools": { "listChanged": false }
        },
        "serverInfo": {
            "name": "anvil-mcp",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

fn dispatch_tool_call(
    server: &mut McpServer,
    id: Value,
    params: Option<&Value>,
) -> JsonRpcResponse {
    let Some(params_val) = params else {
        return JsonRpcResponse::err(id, INVALID_PARAMS, "tools/call requires params");
    };
    let Some(name) = params_val.get("name").and_then(Value::as_str) else {
        return JsonRpcResponse::err(id, INVALID_PARAMS, "tools/call requires string `name`");
    };
    let empty_args = json!({});
    let args = params_val.get("arguments").unwrap_or(&empty_args);

    let outcome = if name.starts_with("graph_") {
        call_graph_tool(server, name, args)
    } else if name.starts_with("mem_") {
        call_memory_tool(server, name, args)
    } else {
        return JsonRpcResponse::err(id, METHOD_NOT_FOUND, format!("unknown tool: {name}"));
    };

    match outcome {
        Ok(payload) => match serde_json::to_string(&payload) {
            Ok(text) => JsonRpcResponse::ok(id, tool_text_result(text, false)),
            Err(err) => JsonRpcResponse::err(id, INTERNAL_ERROR, err.to_string()),
        },
        Err(err) => JsonRpcResponse::ok(id, tool_text_result(format!("{err:#}"), true)),
    }
}

fn tool_text_result(text: String, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    })
}

/// Returns JSON Schema definitions for all 10 unified `graph_*` and `mem_*` tools.
pub fn tool_definitions() -> Vec<Value> {
    let mut tools = graph_tool_schemas();
    tools.extend(memory_tool_schemas());
    tools
}

fn graph_tool_schemas() -> Vec<Value> {
    vec![
        tool_schema(
            "graph_impact",
            "Compute reverse-BFS blast radius and affected tests from changed files.",
            json!({
                "type": "object",
                "properties": {
                    "files": { "type": "array", "items": { "type": "string" }, "description": "Repo-relative changed files (defaults to git working tree diff)." },
                    "depth": { "type": "integer", "minimum": 1, "default": 2 },
                    "typecheck": { "type": "boolean", "default": false }
                }
            }),
        ),
        tool_schema(
            "graph_origin",
            "Trace upstream definition/source-of-truth for anti-slop edits.",
            json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "Symbol name, file path, or graph node ID." }
                },
                "required": ["target"]
            }),
        ),
        tool_schema(
            "graph_query",
            "BM25/token-scored search across code graph symbols, files, and semantic nodes.",
            json!({
                "type": "object",
                "properties": {
                    "term": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 10 }
                },
                "required": ["term"]
            }),
        ),
        tool_schema(
            "graph_path",
            "Find shortest directed dependency path between two symbols or files.",
            json!({
                "type": "object",
                "properties": {
                    "from": { "type": "string" },
                    "to": { "type": "string" }
                },
                "required": ["from", "to"]
            }),
        ),
        tool_schema(
            "graph_explain",
            "Inspect incoming/outgoing neighbors, community, and god-node status for a node.",
            json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string" }
                },
                "required": ["target"]
            }),
        ),
    ]
}

fn memory_tool_schemas() -> Vec<Value> {
    vec![
        mem_save_schema(),
        tool_schema(
            "mem_search",
            "BM25 FTS5 progressive-disclosure memory search returning compact hits.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "kind": { "type": "string", "enum": ["decision", "gotcha", "domain", "session"] },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 10 }
                },
                "required": ["query"]
            }),
        ),
        tool_schema(
            "mem_timeline",
            "Return chronological neighborhood around a memory ID or topic_key.",
            json!({
                "type": "object",
                "properties": {
                    "id_or_topic": { "type": "string" },
                    "before": { "type": "integer", "minimum": 0, "default": 5 },
                    "after": { "type": "integer", "minimum": 0, "default": 5 }
                },
                "required": ["id_or_topic"]
            }),
        ),
        tool_schema(
            "mem_get",
            "Load full observation body by ID (or topic_key fallback).",
            json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
        ),
        tool_schema(
            "mem_context",
            "Load token-budgeted session startup memory context.",
            json!({
                "type": "object",
                "properties": {
                    "max_tokens": { "type": "integer", "minimum": 1, "maximum": 8000, "default": 800 }
                }
            }),
        ),
    ]
}

fn mem_save_schema() -> Value {
    tool_schema(
        "mem_save",
        "Persist or upsert a memory observation (decision, gotcha, domain, or session) with secret redaction.",
        json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": ["decision", "gotcha", "domain", "session"] },
                "title": { "type": "string" },
                "what": { "type": "string" },
                "why": { "type": "string" },
                "where": { "type": "string" },
                "learned": { "type": "string" },
                "topic_key": { "type": "string" },
                "related_files": { "type": "array", "items": { "type": "string" } },
                "related_symbols": { "type": "array", "items": { "type": "string" } },
                "ttl_seconds": { "type": "integer" }
            },
            "required": ["kind", "title", "what", "why", "where", "learned"]
        }),
    )
}

fn tool_schema(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}
