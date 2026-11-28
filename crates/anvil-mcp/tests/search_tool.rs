//! MCP `search` tool and the lookup-order `instructions` sent on `initialize`.

use std::fs;
use std::io::Cursor;

use anvil_core::config::AnvilConfig;
use anvil_graph::CodeGraphEngine;
use anvil_mcp::McpServer;
use anvil_mcp::tools_codemode::CODE_MODE_ALLOWLIST;
use anvil_mcp::transport::{JsonRpcResponse, serve_reader_writer};
use serde_json::{Value, json};
use tempfile::TempDir;

const MATH: &str = "export function add(a: number, b: number): number {\n  return a + b;\n}\n\nexport function multiply(a: number, b: number): number {\n  return a * b;\n}\n";

fn sample_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/math.ts"), MATH).unwrap();
    fs::write(dir.path().join("notes.md"), "Remember the zebra.\n").unwrap();
    dir
}

fn request(server: &mut McpServer, method: &str, params: Value) -> JsonRpcResponse {
    let req = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let mut out = Vec::new();
    serve_reader_writer(Cursor::new(format!("{req}\n").into_bytes()), &mut out, |r| {
        server.handle_message(r)
    })
    .unwrap();
    serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap()
}

fn search(server: &mut McpServer, arguments: Value) -> Value {
    let resp = request(server, "tools/call", json!({ "name": "search", "arguments": arguments }));
    let result = resp.result.expect("result");
    assert_eq!(result["isError"], json!(false), "{result}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn initialize_states_the_lookup_order() {
    let repo = sample_repo();
    let mut server = McpServer::new(repo.path()).unwrap();
    let resp = request(&mut server, "initialize", json!({ "protocolVersion": "2024-11-05" }));
    let text = resp.result.unwrap()["instructions"].as_str().unwrap().to_owned();

    let (graph, docs, grep) = (
        text.find("`search`").unwrap(),
        text.find(".harness/docs/<project>/").unwrap(),
        text.find("`grep`").unwrap(),
    );
    assert!(graph < docs && docs < grep, "order must be graph, docs, grep: {text}");
    for tool in ["graph_query", "graph_outline", "graph_read_symbol"] {
        assert!(text.contains(tool), "{tool} missing: {text}");
    }
    assert!(text.contains("not whole files"), "{text}");
}

#[test]
fn search_tool_is_listed_with_its_description_and_allowed_in_code_mode() {
    let repo = sample_repo();
    let mut server = McpServer::new(repo.path()).unwrap();
    let listed = request(&mut server, "tools/list", json!({}));
    let tools = listed.result.unwrap()["tools"].as_array().unwrap().clone();
    let tool = tools.iter().find(|t| t["name"] == "search").expect("search listed");
    assert_eq!(
        tool["description"],
        "Start here: looks the query up in the code graph, then the living docs, then the code (grep) only if needed."
    );
    assert_eq!(tool["inputSchema"]["required"], json!(["query"]));
    for arg in ["query", "code", "limit", "max_tokens"] {
        assert!(tool["inputSchema"]["properties"][arg].is_object(), "{arg}");
    }
    assert!(CODE_MODE_ALLOWLIST.contains(&"search"));
}

#[test]
fn search_tool_answers_from_the_graph_then_falls_back_to_the_code() {
    let repo = sample_repo();
    CodeGraphEngine::build(repo.path(), AnvilConfig::default()).unwrap().save().unwrap();
    let mut server = McpServer::new(repo.path()).unwrap();

    let graph = search(&mut server, json!({ "query": "multiply" }));
    assert_eq!(graph["answered_by"], json!("graph"));
    assert!(graph["graph_hits"].as_u64().unwrap() >= 1);
    let text = graph["text"].as_str().unwrap();
    assert!(text.contains("symbol multiply  src/math.ts:5"), "{text}");

    let code = search(&mut server, json!({ "query": "zebra" }));
    assert_eq!(code["answered_by"], json!("code"));
    assert_eq!(code["code_matches"], json!(1));
    assert_eq!(code["docs_hits"], json!(0));
    assert!(code["text"].as_str().unwrap().contains("notes.md"));

    let forced = search(&mut server, json!({ "query": "multiply", "code": true }));
    assert!(forced["code_matches"].as_u64().unwrap() >= 1, "{forced}");

    let scripted = search(&mut server, json!({ "query": "add", "max_tokens": 100, "limit": 1 }));
    assert_eq!(scripted["graph_hits"], json!(1));
}

#[test]
fn search_tool_reports_bad_input_and_works_inside_code_mode() {
    let repo = sample_repo();
    let mut server = McpServer::new(repo.path()).unwrap();
    let resp = request(
        &mut server,
        "tools/call",
        json!({ "name": "search", "arguments": { "query": "  " } }),
    );
    assert_eq!(resp.result.unwrap()["isError"], json!(true));

    let scripted = request(
        &mut server,
        "tools/call",
        json!({ "name": "code_exec", "arguments": { "script": "const r = anvil.search({ query: 'zebra' }); return { by: r.answered_by, n: r.code_matches };" } }),
    );
    let result = scripted.result.unwrap();
    assert_eq!(result["isError"], json!(false), "{result}");
    let payload: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["result"], json!({ "by": "code", "n": 1 }));
}
