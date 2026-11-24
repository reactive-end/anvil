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

/// Maximum accepted MCP frame payload size (16 MiB).
pub const MAX_MCP_FRAME_BYTES: usize = 16 * 1024 * 1024;

const DRAIN_CHUNK_BYTES: usize = 8192;
const CONTENT_LENGTH_PREFIX: &str = "content-length:";

enum BoundedLine {
    Eof,
    Line(String),
    Oversize,
}

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

/// Runs a dual-framed (newline-delimited or `Content-Length`) JSON-RPC 2.0 loop over arbitrary buffered streams.
pub fn serve_reader_writer<R, W, F>(reader: R, writer: W, handler: F) -> Result<()>
where
    R: BufRead,
    W: Write,
    F: FnMut(&JsonRpcRequest) -> Option<JsonRpcResponse>,
{
    serve_reader_writer_with_limit(reader, writer, MAX_MCP_FRAME_BYTES, handler)
}

fn serve_reader_writer_with_limit<R, W, F>(
    mut reader: R,
    mut writer: W,
    max_bytes: usize,
    mut handler: F,
) -> Result<()>
where
    R: BufRead,
    W: Write,
    F: FnMut(&JsonRpcRequest) -> Option<JsonRpcResponse>,
{
    loop {
        match read_bounded_line(&mut reader, max_bytes)? {
            BoundedLine::Eof => break,
            BoundedLine::Oversize => write_response(&mut writer, &oversize_error_response())?,
            BoundedLine::Line(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                dispatch_trimmed_line(trimmed, &mut reader, &mut writer, max_bytes, &mut handler)?;
            }
        }
    }
    writer.flush().context("failed to flush JSON-RPC stdout")?;
    Ok(())
}

fn dispatch_trimmed_line<R, W, F>(
    trimmed: &str,
    reader: &mut R,
    writer: &mut W,
    max_bytes: usize,
    handler: &mut F,
) -> Result<()>
where
    R: BufRead,
    W: Write,
    F: FnMut(&JsonRpcRequest) -> Option<JsonRpcResponse>,
{
    if let Some(len_str) = parse_content_length_prefix(trimmed) {
        return handle_content_length_frame(reader, writer, len_str, max_bytes, handler);
    }
    if let Some(resp) = process_line(trimmed, handler) {
        write_response(writer, &resp)?;
    }
    Ok(())
}

fn read_bounded_line<R: BufRead>(reader: &mut R, max_bytes: usize) -> Result<BoundedLine> {
    let mut raw = Vec::new();
    loop {
        let buf = reader.fill_buf().context("failed to read JSON-RPC frame")?;
        if buf.is_empty() {
            return finish_eof_line(raw);
        }
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let take = pos + 1;
            let oversized = raw.len().saturating_add(pos) > max_bytes;
            if !oversized {
                raw.extend_from_slice(&buf[..take]);
            }
            reader.consume(take);
            if oversized {
                return Ok(BoundedLine::Oversize);
            }
            let line = String::from_utf8(raw).context("invalid UTF-8 in JSON-RPC frame")?;
            return Ok(BoundedLine::Line(line));
        }
        let len = buf.len();
        if raw.len().saturating_add(len) > max_bytes {
            reader.consume(len);
            drain_until_newline(reader)?;
            return Ok(BoundedLine::Oversize);
        }
        raw.extend_from_slice(buf);
        reader.consume(len);
    }
}

fn finish_eof_line(raw: Vec<u8>) -> Result<BoundedLine> {
    if raw.is_empty() {
        return Ok(BoundedLine::Eof);
    }
    let line = String::from_utf8(raw).context("invalid UTF-8 in JSON-RPC frame")?;
    Ok(BoundedLine::Line(line))
}

fn drain_until_newline<R: BufRead>(reader: &mut R) -> Result<()> {
    loop {
        let buf = reader.fill_buf().context("failed to drain oversize line")?;
        if buf.is_empty() {
            return Ok(());
        }
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            reader.consume(pos + 1);
            return Ok(());
        }
        let len = buf.len();
        reader.consume(len);
    }
}

fn parse_content_length_prefix(trimmed: &str) -> Option<&str> {
    let head = trimmed.get(..CONTENT_LENGTH_PREFIX.len())?;
    if head.eq_ignore_ascii_case(CONTENT_LENGTH_PREFIX) {
        Some(trimmed[CONTENT_LENGTH_PREFIX.len()..].trim())
    } else {
        None
    }
}

fn consume_header_terminator<R: BufRead>(reader: &mut R, max_bytes: usize) -> Result<bool> {
    loop {
        match read_bounded_line(reader, max_bytes)? {
            BoundedLine::Eof => return Ok(false),
            BoundedLine::Line(hdr) if hdr.trim().is_empty() => return Ok(true),
            BoundedLine::Line(_) | BoundedLine::Oversize => continue,
        }
    }
}

fn drain_exact_bytes<R: BufRead>(reader: &mut R, mut remaining: usize) -> Result<()> {
    let mut discard = [0u8; DRAIN_CHUNK_BYTES];
    while remaining > 0 {
        let chunk_len = remaining.min(discard.len());
        reader
            .read_exact(&mut discard[..chunk_len])
            .context("failed to drain oversize Content-Length frame")?;
        remaining -= chunk_len;
    }
    Ok(())
}

fn handle_content_length_frame<R, W, F>(
    reader: &mut R,
    writer: &mut W,
    len_str: &str,
    max_bytes: usize,
    handler: &mut F,
) -> Result<()>
where
    R: BufRead,
    W: Write,
    F: FnMut(&JsonRpcRequest) -> Option<JsonRpcResponse>,
{
    if !consume_header_terminator(reader, max_bytes)? {
        anyhow::bail!("unexpected EOF while reading Content-Length headers");
    }
    let Ok(len) = len_str.parse::<usize>() else {
        let resp = JsonRpcResponse::err(Value::Null, PARSE_ERROR, "invalid Content-Length header");
        return write_content_length_response(writer, &resp);
    };
    if len > max_bytes {
        drain_exact_bytes(reader, len)?;
        return write_content_length_response(writer, &oversize_error_response());
    }
    if let Some(resp) = read_content_length_body(reader, len, handler)? {
        write_content_length_response(writer, &resp)?;
    }
    Ok(())
}

fn read_content_length_body<R, F>(
    reader: &mut R,
    len: usize,
    handler: &mut F,
) -> Result<Option<JsonRpcResponse>>
where
    R: BufRead,
    F: FnMut(&JsonRpcRequest) -> Option<JsonRpcResponse>,
{
    let mut body = vec![0u8; len];
    reader
        .read_exact(&mut body)
        .context("failed to read Content-Length body")?;
    let Ok(text) = std::str::from_utf8(&body) else {
        return Ok(Some(JsonRpcResponse::err(
            Value::Null,
            PARSE_ERROR,
            "invalid UTF-8 in JSON-RPC frame",
        )));
    };
    Ok(process_line(text.trim(), handler))
}

fn oversize_error_response() -> JsonRpcResponse {
    JsonRpcResponse::err(Value::Null, PARSE_ERROR, "frame exceeds 16 MiB limit")
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

fn write_content_length_response<W: Write>(writer: &mut W, resp: &JsonRpcResponse) -> Result<()> {
    let payload = serde_json::to_vec(resp).context("failed to serialize JSON-RPC response")?;
    write!(writer, "Content-Length: {}\r\n\r\n", payload.len())
        .context("failed to write Content-Length header")?;
    writer
        .write_all(&payload)
        .context("failed to write Content-Length body")?;
    writer
        .flush()
        .context("failed to flush JSON-RPC response")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{self, BufReader, Cursor, Read};

    use serde_json::json;

    use super::*;

    fn echo_handler(req: &JsonRpcRequest) -> Option<JsonRpcResponse> {
        req.id
            .clone()
            .map(|id| JsonRpcResponse::ok(id, json!({ "method": req.method })))
    }

    fn frame_content_length(body: &str, extra_header: Option<&str>) -> String {
        match extra_header {
            Some(hdr) => format!("Content-Length: {}\r\n{hdr}\r\n\r\n{body}", body.len()),
            None => format!("Content-Length: {}\r\n\r\n{body}", body.len()),
        }
    }

    fn parse_content_length_responses(raw: &[u8]) -> Vec<JsonRpcResponse> {
        let mut cursor = BufReader::new(Cursor::new(raw));
        let mut responses = Vec::new();
        loop {
            let mut header = String::new();
            let bytes = cursor.read_line(&mut header).unwrap();
            if bytes == 0 {
                break;
            }
            let len: usize = parse_content_length_prefix(header.trim())
                .expect("expected Content-Length header")
                .parse()
                .unwrap();
            let mut blank = String::new();
            cursor.read_line(&mut blank).unwrap();
            assert!(blank.trim().is_empty());
            let mut body = vec![0u8; len];
            cursor.read_exact(&mut body).unwrap();
            responses.push(serde_json::from_slice(&body).unwrap());
        }
        responses
    }

    #[test]
    fn newline_delimited_requests_still_work() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
        );
        let mut out = Vec::new();
        serve_reader_writer(Cursor::new(input.as_bytes()), &mut out, echo_handler).unwrap();

        let text = String::from_utf8(out).unwrap();
        let lines: Vec<JsonRpcResponse> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].id, json!(1));
        assert_eq!(lines[0].result, Some(json!({ "method": "ping" })));
        assert_eq!(lines[1].id, json!(2));
        assert_eq!(lines[1].result, Some(json!({ "method": "tools/list" })));
    }

    #[test]
    fn content_length_single_and_back_to_back_roundtrip() {
        let req1 = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let notif = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let req2 = r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#;
        let hdr = Some("Content-Type: application/vscode-jsonrpc; charset=utf-8");

        let mut single_out = Vec::new();
        let single_in = frame_content_length(req1, hdr);
        serve_reader_writer(Cursor::new(single_in.as_bytes()), &mut single_out, echo_handler)
            .unwrap();
        let single_resps = parse_content_length_responses(&single_out);
        assert_eq!(single_resps.len(), 1);
        assert_eq!(single_resps[0].id, json!(1));
        assert_eq!(single_resps[0].result, Some(json!({ "method": "initialize" })));

        let batch_in = format!(
            "{}{}{}",
            frame_content_length(req1, hdr),
            frame_content_length(notif, hdr),
            frame_content_length(req2, None),
        );
        let mut batch_out = Vec::new();
        serve_reader_writer(Cursor::new(batch_in.as_bytes()), &mut batch_out, echo_handler)
            .unwrap();
        let batch_resps = parse_content_length_responses(&batch_out);
        assert_eq!(batch_resps.len(), 2);
        assert_eq!(batch_resps[0].id, json!(1));
        assert_eq!(batch_resps[1].id, json!(2));
        assert_eq!(batch_resps[1].result, Some(json!({ "method": "ping" })));
    }

    #[test]
    fn oversize_content_length_returns_parse_error_without_oom() {
        let oversize_len = 20_000_000usize;
        let header = format!(
            "content-length: {oversize_len}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n"
        );
        let followup = frame_content_length(r#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#, None);
        let stream = Cursor::new(header.into_bytes())
            .chain(io::repeat(b'a').take(oversize_len as u64))
            .chain(Cursor::new(followup.into_bytes()));

        let mut out = Vec::new();
        serve_reader_writer(BufReader::new(stream), &mut out, echo_handler).unwrap();

        let resps = parse_content_length_responses(&out);
        assert_eq!(resps.len(), 2);
        assert_eq!(resps[0].id, Value::Null);
        assert_eq!(resps[0].error.as_ref().unwrap().code, PARSE_ERROR);
        assert_eq!(
            resps[0].error.as_ref().unwrap().message,
            "frame exceeds 16 MiB limit"
        );
        assert_eq!(resps[1].id, json!(9));
        assert_eq!(resps[1].result, Some(json!({ "method": "ping" })));
    }

    #[test]
    fn oversize_newline_frame_returns_parse_error_and_recovers() {
        let huge_line = format!("{}\n", "x".repeat(256));
        let valid_line = "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n";
        let input = format!("{huge_line}{valid_line}");

        let mut out = Vec::new();
        serve_reader_writer_with_limit(Cursor::new(input.as_bytes()), &mut out, 64, echo_handler)
            .unwrap();

        let text = String::from_utf8(out).unwrap();
        let resps: Vec<JsonRpcResponse> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(resps.len(), 2);
        assert_eq!(resps[0].id, Value::Null);
        assert_eq!(resps[0].error.as_ref().unwrap().code, PARSE_ERROR);
        assert_eq!(
            resps[0].error.as_ref().unwrap().message,
            "frame exceeds 16 MiB limit"
        );
        assert_eq!(resps[1].id, json!(7));
        assert_eq!(resps[1].result, Some(json!({ "method": "ping" })));
    }
}
