//! MCP lifecycle routing, tool schemas, and tool call dispatch.

use serde_json::{Value, json};

use crate::McpServer;
use crate::tools_codemode::call_code_exec;
use crate::tools_graph::call_graph_tool;
use crate::tools_memory::call_memory_tool;
use crate::tools_review::{call_review_approve, call_review_log, call_review_packet};
use crate::tools_tasks::call_task_tool;
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

    let outcome = if name == "code_exec" {
        call_code_exec(server, args)
    } else if name == "context_pack" || name.starts_with("graph_") {
        call_graph_tool(server, name, args)
    } else if name == "review_log" {
        call_review_log(server, args)
    } else if name == "review_packet" {
        call_review_packet(server, args)
    } else if name == "review_approve" {
        call_review_approve(server, args)
    } else if name.starts_with("mem_") {
        call_memory_tool(server, name, args)
    } else if name.starts_with("task_") {
        call_task_tool(server, name, args)
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

pub use crate::schemas::tool_definitions;

fn tool_text_result(text: String, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    })
}
