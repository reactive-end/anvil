use std::fs;
use std::io::Cursor;

use anvil_mcp::McpServer;
use anvil_mcp::transport::{JsonRpcResponse, serve_reader_writer};
use serde_json::{Value, json};
use tempfile::TempDir;

fn setup_sample_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("math.ts"),
        "export function add(a: number, b: number): number {\n  return a + b;\n}\n",
    )
    .unwrap();
    fs::write(src.join("index.ts"), "export { add } from './math';\n").unwrap();
    fs::write(
        src.join("app.ts"),
        "import { add } from './index';\nexport function runApp(): number {\n  return add(1, 2);\n}\n",
    )
    .unwrap();
    dir
}

fn run_session(server: &mut McpServer, requests: &[Value]) -> Vec<JsonRpcResponse> {
    let mut input = String::new();
    for req in requests {
        input.push_str(&serde_json::to_string(req).unwrap());
        input.push('\n');
    }
    let reader = Cursor::new(input.into_bytes());
    let mut out_bytes = Vec::new();
    serve_reader_writer(reader, &mut out_bytes, |req| server.handle_message(req)).unwrap();

    let out_str = String::from_utf8(out_bytes).unwrap();
    out_str
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<JsonRpcResponse>(l).unwrap())
        .collect()
}

fn parse_tool_json(resp: &JsonRpcResponse) -> Value {
    let result = resp.result.as_ref().expect("expected ok result");
    assert_eq!(result["isError"], json!(false), "unexpected tool error: {result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}

#[test]
fn mcp_initialize_and_tools_list() {
    let repo = setup_sample_repo();
    let mut server = McpServer::new(repo.path()).unwrap();
    let resps = run_session(
        &mut server,
        &[
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2024-11-05" }
            }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
        ],
    );

    assert_eq!(resps.len(), 3);
    assert!(server.is_initialized());
    assert_eq!(resps[0].result.as_ref().unwrap()["serverInfo"]["name"], "anvil-mcp");
    assert_eq!(resps[1].result, Some(json!({})));

    let tools = resps[2].result.as_ref().unwrap()["tools"]
        .as_array()
        .unwrap();
    assert_eq!(tools.len(), 10);
    assert!(tools.iter().all(|t| t["inputSchema"].is_object()));
}

fn call_req(id: i64, name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    })
}

#[test]
fn mcp_memory_tools_roundtrip() {
    let repo = setup_sample_repo();
    let mut server = McpServer::new(repo.path()).unwrap();
    let initial = run_session(
        &mut server,
        &[
            call_req(
                1,
                "mem_save",
                json!({
                    "kind": "decision",
                    "title": "Use pure math module",
                    "what": "Keep add pure without side effects",
                    "why": "Deterministic tests and caching",
                    "where": "src/math.ts",
                    "learned": "No IO in math helpers",
                    "related_files": ["src/math.ts"],
                    "related_symbols": ["add"]
                }),
            ),
            call_req(2, "mem_search", json!({ "query": "pure math", "limit": 5 })),
        ],
    );

    let obs_id = parse_tool_json(&initial[0])["observation"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(parse_tool_json(&initial[1])["hits"][0]["id"], json!(obs_id));

    let followups = run_session(
        &mut server,
        &[
            call_req(3, "mem_get", json!({ "id": obs_id })),
            call_req(4, "mem_timeline", json!({ "id_or_topic": obs_id })),
            call_req(5, "mem_context", json!({ "max_tokens": 500 })),
        ],
    );
    assert_eq!(parse_tool_json(&followups[0])["found"], json!(true));
    assert!(parse_tool_json(&followups[1])["count"].as_u64().unwrap() >= 1);
    assert!(parse_tool_json(&followups[2])["count"].as_u64().unwrap() >= 1);
}

#[test]
fn mcp_graph_tools_roundtrip() {
    let repo = setup_sample_repo();
    let mut server = McpServer::new(repo.path()).unwrap();
    let resps = run_session(
        &mut server,
        &[
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": "graph_query", "arguments": { "term": "add", "limit": 5 } }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": { "name": "graph_impact", "arguments": { "files": ["src/math.ts"], "depth": 2 } }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": { "name": "graph_origin", "arguments": { "target": "src/index.ts" } }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": { "name": "graph_path", "arguments": { "from": "src/app.ts", "to": "src/math.ts" } }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": { "name": "graph_explain", "arguments": { "target": "src/math.ts" } }
            }),
        ],
    );

    assert!(parse_tool_json(&resps[0])["count"].as_u64().unwrap() >= 1);
    let impact = parse_tool_json(&resps[1]);
    let impacted = impact["blast_radius"]["impacted_files"].as_array().unwrap();
    assert!(impacted.iter().any(|item| item["file"] == "src/app.ts"));
    assert_eq!(parse_tool_json(&resps[2])["origin"]["origin_file"], json!("src/math.ts"));
    assert_eq!(parse_tool_json(&resps[3])["found"], json!(true));
    assert_eq!(parse_tool_json(&resps[4])["found"], json!(true));
}

#[test]
fn mcp_memory_syncs_new_external_chunk_on_chunks_dir_mtime_change() {
    let repo = setup_sample_repo();
    let mut server = McpServer::new(repo.path()).unwrap();

    let first = run_session(
        &mut server,
        &[call_req(1, "mem_search", json!({ "query": "external chunk", "limit": 5 }))],
    );
    assert_eq!(parse_tool_json(&first[0])["count"], json!(0));

    let chunks_dir = repo.path().join(".harness/memory/chunks");
    fs::create_dir_all(&chunks_dir).unwrap();
    let chunk = json!({
        "id": "01J00000000000000000000002",
        "topic_key": "gotcha/external-chunk",
        "kind": "gotcha",
        "title": "External chunk arrival",
        "what": "Teammate pulled a new memory chunk",
        "why": "MCP server must detect chunks dir mtime change",
        "location": "src/math.ts",
        "learned": "Check chunks dir mtime before skipping sync",
        "related_files": ["src/math.ts"],
        "related_symbols": [],
        "author": "teammate",
        "created_at": 1700000000000i64,
        "updated_at": 1700000000000i64,
        "expires_at": null
    });
    fs::write(chunks_dir.join("teammate-01J00000000000000000000002.jsonl"), format!("{chunk}\n")).unwrap();

    let second = run_session(
        &mut server,
        &[call_req(2, "mem_search", json!({ "query": "external chunk", "limit": 5 }))],
    );
    assert_eq!(parse_tool_json(&second[0])["count"], json!(1));
}

#[test]
fn mcp_negotiates_protocol_version_and_emits_compact_tool_json() {
    let repo = setup_sample_repo();
    let mut server = McpServer::new(repo.path()).unwrap();
    let resps = run_session(
        &mut server,
        &[
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-03-26" }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "initialize"
            }),
            call_req(3, "graph_query", json!({ "term": "add", "limit": 3 })),
        ],
    );
    assert_eq!(resps[0].result.as_ref().unwrap()["protocolVersion"], json!("2025-03-26"));
    assert_eq!(resps[1].result.as_ref().unwrap()["protocolVersion"], json!("2024-11-05"));
    let text = resps[2].result.as_ref().unwrap()["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(!text.contains('\n'), "expected compact JSON without newlines, got: {text}");
}

#[test]
fn mcp_mem_save_before_read_still_syncs_preexisting_external_chunk() {
    let repo = setup_sample_repo();
    let chunks_dir = repo.path().join(".harness/memory/chunks");
    fs::create_dir_all(&chunks_dir).unwrap();
    let chunk = json!({
        "id": "01J00000000000000000000009",
        "topic_key": "decision/preexisting-team-chunk",
        "kind": "decision",
        "title": "Preexisting teammate insight",
        "what": "Already on disk before MCP server first saves",
        "why": "mem_save must never mask initial chunk sync",
        "location": "src/math.ts",
        "learned": "Keep chunks_synced_once false until sync runs",
        "related_files": ["src/math.ts"],
        "related_symbols": [],
        "author": "teammate",
        "created_at": 1700000000000i64,
        "updated_at": 1700000000000i64,
        "expires_at": null
    });
    fs::write(
        chunks_dir.join("teammate-01J00000000000000000000009.jsonl"),
        format!("{chunk}\n"),
    )
    .unwrap();

    let mut server = McpServer::new(repo.path()).unwrap();
    let resps = run_session(
        &mut server,
        &[
            call_req(
                1,
                "mem_save",
                json!({
                    "kind": "decision",
                    "title": "Local save first",
                    "what": "Save before any read tool",
                    "why": "Repro sync-masking P0",
                    "where": "src/app.ts",
                    "learned": "Must not skip teammate chunk sync"
                }),
            ),
            call_req(2, "mem_search", json!({ "query": "Preexisting teammate", "limit": 5 })),
        ],
    );
    assert_eq!(parse_tool_json(&resps[0])["saved"], json!(true));
    assert_eq!(parse_tool_json(&resps[1])["count"], json!(1));
}
