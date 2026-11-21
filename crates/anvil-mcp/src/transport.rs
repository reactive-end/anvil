//! Buffered stdio JSON-RPC 2.0 transport for the unified `anvil-mcp` server.

use std::borrow::Cow;
use std::io::{self, BufRead, BufReader, BufWriter, Write};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const JSONRPC_VERSION: &str = "2.0";
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

/// Incoming JSON-RPC 2.0 request or notification.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

/// Outgoing JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: Cow<'static, str>,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// Structured JSON-RPC 2.0 error payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    /// Constructs a success response for `id`.
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: Cow::Borrowed(JSONRPC_VERSION),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Constructs an error response for `id`.
    pub fn err(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: Cow::Borrowed(JSONRPC_VERSION),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

/// Runs a newline-delimited JSON-RPC 2.0 loop over process stdin/stdout with buffered I/O.
pub fn run_stdio_loop<F>(handler: F) -> Result<()>
where
    F: FnMut(&JsonRpcRequest) -> Option<JsonRpcResponse>,
{
    let stdin = io::stdin();
    let stdout = io::stdout();
    let reader = BufReader::with_capacity(64 * 1024, stdin.lock());
    let writer = BufWriter::with_capacity(64 * 1024, stdout.lock());
    serve_reader_writer(reader, writer, handler)
}

/// Runs a newline-delimited JSON-RPC 2.0 loop over arbitrary buffered reader/writer streams.
pub fn serve_reader_writer<R, W, F>(
    mut reader: R,
    mut writer: W,
    mut handler: F,
) -> Result<()>
where
    R: BufRead,
    W: Write,
    F: FnMut(&JsonRpcRequest) -> Option<JsonRpcResponse>,
{
    let mut line = String::with_capacity(1024);
    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .context("failed to read JSON-RPC frame")?;
        if bytes == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(resp) = process_line(trimmed, &mut handler) {
            write_response(&mut writer, &resp)?;
        }
    }
    writer.flush().context("failed to flush JSON-RPC stdout")?;
    Ok(())
}

fn process_line<F>(trimmed: &str, handler: &mut F) -> Option<JsonRpcResponse>
where
    F: FnMut(&JsonRpcRequest) -> Option<JsonRpcResponse>,
{
    let req: JsonRpcRequest = match serde_json::from_str(trimmed) {
        Ok(parsed) => parsed,
        Err(err) => {
            return Some(JsonRpcResponse::err(
                Value::Null,
                PARSE_ERROR,
                format!("parse error: {err}"),
            ));
        }
    };

    if req.jsonrpc != JSONRPC_VERSION {
        return req.id.map(|id| {
            JsonRpcResponse::err(id, INVALID_REQUEST, "jsonrpc field must be \"2.0\"")
        });
    }
    handler(&req)
}

fn write_response<W: Write>(writer: &mut W, resp: &JsonRpcResponse) -> Result<()> {
    serde_json::to_writer(&mut *writer, resp).context("failed to serialize JSON-RPC response")?;
    writer
        .write_all(b"\n")
        .context("failed to write response newline")?;
    writer
        .flush()
        .context("failed to flush JSON-RPC response")?;
    Ok(())
}
