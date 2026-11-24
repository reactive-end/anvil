//! Code Mode MCP Proxy Gateway wrapping external stdio MCP servers behind `proxy_search` + `proxy_exec`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use anvil_codemode::{CodeModePolicy, ToolDispatcher, execute_script};
use anvil_memory::redactor::redact_text;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::transport::{
    INTERNAL_ERROR, INVALID_PARAMS, JsonRpcRequest, JsonRpcResponse, METHOD_NOT_FOUND, run_stdio_loop,
};

const PROXY_CONFIG_REL: &str = ".harness/mcp-proxy.json";
const STRICT_MUTATING_TOKENS: &[&str] = &[
    "append",
    "assign",
    "commit",
    "create",
    "delete",
    "deploy",
    "destroy",
    "drop",
    "edit",
    "eval",
    "exec",
    "execute",
    "grant",
    "insert",
    "install",
    "kill",
    "merge",
    "modify",
    "mutate",
    "patch",
    "publish",
    "purge",
    "push",
    "remove",
    "reset",
    "revoke",
    "send",
    "transition",
    "uninstall",
    "update",
    "upload",
    "wipe",
    "write",
];

const MUTATION_TOKENS: &[&str] = &[
    "add",
    "apply",
    "approve",
    "archive",
    "cancel",
    "clear",
    "close",
    "comment",
    "disable",
    "done",
    "enable",
    "invoke",
    "move",
    "post",
    "put",
    "rank",
    "rebase",
    "record",
    "reject",
    "rename",
    "restart",
    "revert",
    "run",
    "save",
    "set",
    "skip",
    "start",
    "stop",
    "trigger",
];

const READ_ONLY_VERBS: &[&str] = &[
    "check",
    "context",
    "count",
    "describe",
    "diff",
    "explain",
    "fetch",
    "find",
    "get",
    "health",
    "help",
    "impact",
    "inspect",
    "list",
    "log",
    "logs",
    "origin",
    "outline",
    "path",
    "ping",
    "query",
    "read",
    "search",
    "show",
    "stat",
    "stats",
    "status",
    "timeline",
    "version",
    "view",
];

/// Configuration file schema loaded from `.harness/mcp-proxy.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default)]
    pub servers: BTreeMap<String, ProxyServerSpec>,
}

/// Upstream stdio MCP server specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyServerSpec {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub allow_mutations: bool,
}

#[derive(Debug, Clone)]
struct UpstreamTool {
    server: String,
    name: String,
    description: String,
    signature: String,
}

struct UpstreamClient {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    rx: Receiver<std::io::Result<Option<String>>>,
    next_id: i64,
}

impl Drop for UpstreamClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl UpstreamClient {
    fn spawn(root: &Path, spec: &ProxyServerSpec) -> Result<Self> {
        let mut cmd = Command::new(&spec.command);
        cmd.args(&spec.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn upstream MCP server `{}`", spec.command))?;
        let stdin = BufWriter::new(
            child
                .stdin
                .take()
                .ok_or_else(|| anyhow::anyhow!("missing upstream stdin"))?,
        );
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("missing upstream stdout"))?;
        let rx = spawn_stdout_reader(stdout);
        let mut client = Self {
            child,
            stdin,
            rx,
            next_id: 1,
        };
        let _ = client.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "anvil-mcp-proxy", "version": env!("CARGO_PKG_VERSION") }
            }),
        )?;
        client.notify("notifications/initialized", json!({}))?;
        Ok(client)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let line = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))?;
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let line = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))?;
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()?;

        let deadline = Instant::now() + proxy_request_timeout();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("upstream MCP server timed out waiting for `{method}`");
            }
            let raw_line = match self.rx.recv_timeout(remaining) {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) => {
                    bail!("upstream MCP server closed stdout before responding to `{method}`");
                }
                Ok(Err(err)) => return Err(err.into()),
                Err(_) => bail!("upstream MCP server timed out waiting for `{method}`"),
            };
            let trimmed = raw_line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let val: Value = serde_json::from_str(trimmed)?;
            if val.get("id").and_then(Value::as_i64) == Some(id) {
                if let Some(err) = val.get("error") {
                    bail!("upstream MCP error on `{method}`: {err}");
                }
                return Ok(val.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }
}

fn spawn_stdout_reader(stdout: ChildStdout) -> Receiver<std::io::Result<Option<String>>> {
    let (tx, rx) = mpsc::sync_channel::<std::io::Result<Option<String>>>(64);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(Ok(None));
                    break;
                }
                Ok(_) => {
                    if tx.send(Ok(Some(buf.clone()))).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    let _ = tx.send(Err(err));
                    break;
                }
            }
        }
    });
    rx
}

fn proxy_request_timeout() -> Duration {
    let secs = std::env::var("ANVIL_MCP_PROXY_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(30);
    Duration::from_secs(secs)
}

/// Code Mode MCP gateway wrapping external MCP servers behind `proxy_search` + `proxy_exec`.
pub struct ProxyGateway {
    repo_root: PathBuf,
    config: ProxyConfig,
    clients: BTreeMap<String, UpstreamClient>,
    catalog: Vec<UpstreamTool>,
    catalog_loaded: bool,
}

impl ProxyGateway {
    /// Creates a gateway reading `.harness/mcp-proxy.json` if present.
    pub fn new(repo_root: impl AsRef<Path>) -> Result<Self> {
        let root = repo_root.as_ref().to_path_buf();
        let cfg_path = root.join(PROXY_CONFIG_REL);
        let config = if cfg_path.is_file() {
            let raw = fs::read_to_string(&cfg_path)?;
            serde_json::from_str(&raw)
                .with_context(|| format!("failed to parse {}", cfg_path.display()))?
        } else {
            ProxyConfig::default()
        };
        Ok(Self {
            repo_root: root,
            config,
            clients: BTreeMap::new(),
            catalog: Vec::new(),
            catalog_loaded: false,
        })
    }

    /// Runs the stdio MCP proxy loop until EOF.
    pub fn run_stdio(&mut self) -> Result<()> {
        run_stdio_loop(|req| self.handle_message(req))
    }

    /// Handles a single JSON-RPC request/notification.
    pub fn handle_message(&mut self, req: &JsonRpcRequest) -> Option<JsonRpcResponse> {
        let id = req.id.clone()?;
        let resp = match req.method.as_str() {
            "initialize" => JsonRpcResponse::ok(
                id,
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "anvil-mcp-proxy", "version": env!("CARGO_PKG_VERSION") }
                }),
            ),
            "ping" => JsonRpcResponse::ok(id, json!({})),
            "tools/list" => JsonRpcResponse::ok(id, json!({ "tools": proxy_tool_definitions() })),
            "tools/call" => self.dispatch_call(id, req.params.as_ref()),
            other => JsonRpcResponse::err(id, METHOD_NOT_FOUND, format!("unknown method: {other}")),
        };
        Some(resp)
    }

    fn dispatch_call(&mut self, id: Value, params: Option<&Value>) -> JsonRpcResponse {
        let Some(p) = params else {
            return JsonRpcResponse::err(id, INVALID_PARAMS, "tools/call requires params");
        };
        let Some(name) = p.get("name").and_then(Value::as_str) else {
            return JsonRpcResponse::err(id, INVALID_PARAMS, "tools/call requires string `name`");
        };
        let empty = json!({});
        let args = p.get("arguments").unwrap_or(&empty);
        let outcome = match name {
            "proxy_search" => self.handle_search(args),
            "proxy_exec" => self.handle_exec(args),
            other => return JsonRpcResponse::err(id, METHOD_NOT_FOUND, format!("unknown tool: {other}")),
        };
        match outcome {
            Ok(payload) => match serde_json::to_string(&payload) {
                Ok(text) => JsonRpcResponse::ok(
                    id,
                    json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
                ),
                Err(e) => JsonRpcResponse::err(id, INTERNAL_ERROR, e.to_string()),
            },
            Err(err) => JsonRpcResponse::ok(
                id,
                json!({ "content": [{ "type": "text", "text": format!("{err:#}") }], "isError": true }),
            ),
        }
    }

    fn ensure_catalog(&mut self) -> Result<()> {
        if self.catalog_loaded {
            return Ok(());
        }
        let specs: Vec<(String, ProxyServerSpec)> = self
            .config
            .servers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (name, spec) in specs {
            self.ensure_client(&name, &spec)?;
            let list_res = self
                .clients
                .get_mut(&name)
                .ok_or_else(|| anyhow::anyhow!("missing client"))?
                .request("tools/list", json!({}))?;
            if let Some(arr) = list_res.get("tools").and_then(Value::as_array) {
                for tool in arr {
                    let tname = tool["name"].as_str().unwrap_or("").to_owned();
                    let desc = tool["description"].as_str().unwrap_or("").to_owned();
                    let sig = format_ts_signature(&name, &tname, &tool["inputSchema"]);
                    self.catalog.push(UpstreamTool {
                        server: name.clone(),
                        name: tname,
                        description: desc,
                        signature: sig,
                    });
                }
            }
        }
        self.catalog_loaded = true;
        Ok(())
    }

    fn ensure_client(&mut self, name: &str, spec: &ProxyServerSpec) -> Result<&mut UpstreamClient> {
        if !self.clients.contains_key(name) {
            let client = UpstreamClient::spawn(&self.repo_root, spec)?;
            self.clients.insert(name.to_owned(), client);
        }
        self.clients
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("missing upstream client `{name}`"))
    }

    fn handle_search(&mut self, args: &Value) -> Result<Value> {
        self.ensure_catalog()?;
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map_or(15, |v| (v as usize).clamp(1, 50));

        let matches: Vec<Value> = self
            .catalog
            .iter()
            .filter(|t| {
                query.is_empty()
                    || t.server.to_ascii_lowercase().contains(&query)
                    || t.name.to_ascii_lowercase().contains(&query)
                    || t.description.to_ascii_lowercase().contains(&query)
            })
            .take(limit)
            .map(|t| {
                json!({
                    "server": t.server,
                    "tool": t.name,
                    "signature": t.signature,
                    "description": t.description,
                })
            })
            .collect();
        Ok(json!({ "total_tools": self.catalog.len(), "matches": matches }))
    }

    fn handle_exec(&mut self, args: &Value) -> Result<Value> {
        let script = args
            .get("script")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("`script` string is required"))?;
        let wrapped = format!(
            "const proxy = {{ call: (server, tool, params) => anvil.call('proxy_call', {{ server, tool, arguments: params || {{}} }}) }};\n{script}"
        );
        let mut dispatcher = ProxyDispatcher(self);
        let res = execute_script(&wrapped, &["proxy_call"], &mut dispatcher, &CodeModePolicy::default())?;
        Ok(json!({
            "result": res.value,
            "calls_executed": res.calls_executed,
            "intermediate_bytes": res.intermediate_bytes,
            "output_bytes": res.output_bytes,
            "estimated_tokens_saved": res.estimated_tokens_saved,
        }))
    }

    fn invoke_upstream(&mut self, server: &str, tool: &str, arguments: Value) -> Result<Value> {
        let Some(spec) = self.config.servers.get(server).cloned() else {
            bail!("unknown upstream MCP server `{server}`");
        };
        if !spec.allow_mutations && is_mutating_tool_name(tool) {
            bail!(
                "blocked mutating upstream tool `{server}.{tool}` (`allow_mutations` is false in {PROXY_CONFIG_REL})"
            );
        }
        let client = self.ensure_client(server, &spec)?;
        let res = client.request("tools/call", json!({ "name": tool, "arguments": arguments }))?;
        if res.get("isError").and_then(Value::as_bool) == Some(true) {
            let msg = res["content"]
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|item| item.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("upstream tool returned error");
            bail!("upstream tool `{server}.{tool}` failed: {}", redact_text(msg));
        }
        Ok(redact_json_secrets(unwrap_mcp_tool_result(res)))
    }
}

fn unwrap_mcp_tool_result(res: Value) -> Value {
    if let Some(arr) = res.get("content").and_then(Value::as_array)
        && arr.len() == 1
        && let Some(text) = arr[0].get("text").and_then(Value::as_str)
    {
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            return parsed;
        }
        return json!({ "text": text });
    }
    res
}

struct ProxyDispatcher<'a>(&'a mut ProxyGateway);

impl ToolDispatcher for ProxyDispatcher<'_> {
    fn call(&mut self, name: &str, args: Value) -> Result<Value> {
        if name != "proxy_call" {
            bail!("unexpected proxy tool `{name}`");
        }
        let server = args
            .get("server")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("`server` is required"))?;
        let tool = args
            .get("tool")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("`tool` is required"))?;
        let arguments = args.get("arguments").cloned().unwrap_or_else(|| json!({}));
        self.0.invoke_upstream(server, tool, arguments)
    }
}

fn proxy_tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "proxy_search",
            "description": "Search upstream MCP tools configured in `.harness/mcp-proxy.json` and return compact TypeScript signatures.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 15 }
                }
            }
        }),
        json!({
            "name": "proxy_exec",
            "description": "Run a sandboxed Code Mode JS script using `proxy.call(server, tool, args)` with secret redaction and mutation guards.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": { "type": "string" }
                },
                "required": ["script"]
            }
        }),
    ]
}

/// Formats an upstream JSON Schema into a concise single-line TypeScript signature.
pub fn format_ts_signature(server: &str, tool: &str, schema: &Value) -> String {
    let params = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|props| {
            props
                .iter()
                .map(|(k, v)| {
                    let ty = v.get("type").and_then(Value::as_str).unwrap_or("any");
                    format!("{k}?: {ty}")
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    format!("proxy.call('{server}', '{tool}', {{ {params} }})")
}

/// Returns `true` when `tool` appears mutating/destructive and requires `allow_mutations = true`.
pub fn is_mutating_tool_name(tool: &str) -> bool {
    let bare = tool.rsplit(['.', ':']).next().unwrap_or(tool);
    let bare_tokens = tokenize_tool_name(bare);
    let all_tokens = tokenize_tool_name(tool);
    if all_tokens
        .iter()
        .any(|t| STRICT_MUTATING_TOKENS.contains(&t.as_str()))
    {
        return true;
    }
    if bare_tokens
        .first()
        .is_some_and(|lead| READ_ONLY_VERBS.contains(&lead.as_str()))
    {
        return false;
    }
    all_tokens
        .iter()
        .any(|t| MUTATION_TOKENS.contains(&t.as_str()))
}

fn tokenize_tool_name(tool: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for segment in tool.split(['_', '-', '.', ':', '/']).filter(|s| !s.is_empty()) {
        let chars: Vec<char> = segment.chars().collect();
        let mut cur = String::new();
        for (i, &ch) in chars.iter().enumerate() {
            let prev_lower = i > 0 && chars[i - 1].is_ascii_lowercase();
            let next_lower = chars.get(i + 1).is_some_and(|c| c.is_ascii_lowercase());
            if !cur.is_empty() && ch.is_ascii_uppercase() && (prev_lower || next_lower) {
                tokens.push(cur.to_ascii_lowercase());
                cur.clear();
            }
            cur.push(ch);
        }
        if !cur.is_empty() {
            tokens.push(cur.to_ascii_lowercase());
        }
    }
    tokens
}

/// Recursively redacts secrets from any JSON strings returned by upstream MCP servers.
pub fn redact_json_secrets(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(redact_text(&s)),
        Value::Array(items) => Value::Array(items.into_iter().map(redact_json_secrets).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, redact_json_secrets(v)))
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_detection_and_secret_redaction_and_ts_sig() {
        for (tool, expected) in [
            ("create_issue", true),
            ("createIssue", true),
            ("deleteUser", true),
            ("jira.transitionIssue", true),
            ("executeSql", true),
            ("deployService", true),
            ("uploadFile", true),
            ("get_issue", false),
            ("getPost", false),
            ("jira.getPost", false),
            ("read_run_log", false),
            ("list_posts", false),
        ] {
            assert_eq!(is_mutating_tool_name(tool), expected, "tool={tool}");
        }

        let sig = format_ts_signature(
            "anvil",
            "graph_query",
            &json!({ "properties": { "term": { "type": "string" } } }),
        );
        assert!(sig.contains("proxy.call('anvil', 'graph_query'"));
        assert!(sig.contains("term?: string"));

        let cleaned = redact_json_secrets(json!({
            "msg": "token=ghp_123456789012345678901234567890123456 ok"
        }));
        assert!(!cleaned["msg"].as_str().unwrap().contains("ghp_"));
    }
}
