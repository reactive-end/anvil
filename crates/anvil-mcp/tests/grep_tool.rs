//! MCP `grep` tool: router dispatch, grouped text, symbol annotation, argument handling, Code Mode.

use std::fs;
use std::io::Cursor;

use anvil_core::config::AnvilConfig;
use anvil_graph::CodeGraphEngine;
use anvil_mcp::McpServer;
use anvil_mcp::transport::{JsonRpcResponse, serve_reader_writer};
use serde_json::{Value, json};
use tempfile::TempDir;

const MATH: &str = "export function add(a: number, b: number): number {\n  return a + b;\n}\n\nexport function multiply(a: number, b: number): number {\n  return a * b;\n}\n";

fn sample_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/math.ts"), MATH).unwrap();
    fs::write(dir.path().join("src/app.ts"), "import { add } from './math';\nadd(1, 2);\n")
        .unwrap();
    fs::write(dir.path().join("notes.md"), "We add numbers.\n").unwrap();
    dir
}

fn call(server: &mut McpServer, name: &str, arguments: Value) -> JsonRpcResponse {
    let req = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    });
    let input = format!("{req}\n");
    let mut out = Vec::new();
    serve_reader_writer(Cursor::new(input.into_bytes()), &mut out, |r| server.handle_message(r))
        .unwrap();
    serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap()
}

fn payload(resp: &JsonRpcResponse) -> Value {
    let result = resp.result.as_ref().expect("result");
    assert_eq!(result["isError"], json!(false), "{result}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn grep_returns_text_grouped_by_file_with_totals() {
    let repo = sample_repo();
    let mut server = McpServer::new(repo.path()).unwrap();
    let out = payload(&call(&mut server, "grep", json!({ "pattern": "add" })));

    assert_eq!(
        out["text"],
        json!(
            "notes.md\n  1: We add numbers.\nsrc/app.ts\n  1: import { add } from './math';\n  2: add(1, 2);\nsrc/math.ts\n  1: export function add(a: number, b: number): number {\n# grep: 4 of 4 match(es) in 3 file(s), searched 3 file(s), backend=builtin"
        )
    );
    assert_eq!((out["total_matches"].clone(), out["truncated"].clone()), (json!(4), json!(false)));
    assert_eq!(out["backend"], json!("builtin"));
}

#[test]
fn grep_honours_paths_glob_context_and_limits() {
    let repo = sample_repo();
    let mut server = McpServer::new(repo.path()).unwrap();

    let scoped = payload(&call(
        &mut server,
        "grep",
        json!({ "pattern": "return", "paths": ["src"], "glob": "*.ts", "context": 1, "word": true }),
    ));
    let text = scoped["text"].as_str().unwrap();
    assert!(text.starts_with("src/math.ts\n  1- export function add"), "{text}");
    assert!(
        text.contains("  2: return a + b;\n  3- }\n  --\n  5- export function multiply"),
        "{text}"
    );
    assert_eq!(scoped["total_matches"], json!(2));

    let globs = payload(&call(&mut server, "grep", json!({ "pattern": "add", "glob": ["*.md"] })));
    assert_eq!(globs["files_with_matches"], json!(1));

    let capped = payload(&call(&mut server, "grep", json!({ "pattern": "add", "max_matches": 1 })));
    assert_eq!(capped["truncated"], json!(true));
    assert_eq!(capped["total_matches"], json!(4), "totals stay exact");

    let fixed =
        payload(&call(&mut server, "grep", json!({ "pattern": "(1, 2)", "fixed_strings": true })));
    assert_eq!(fixed["total_matches"], json!(1));
    let cased =
        payload(&call(&mut server, "grep", json!({ "pattern": "WE", "ignore_case": true })));
    assert_eq!(cased["total_matches"], json!(1));
}

#[test]
fn grep_reports_bad_input_as_a_tool_error() {
    let repo = sample_repo();
    let mut server = McpServer::new(repo.path()).unwrap();
    for arguments in
        [json!({}), json!({ "pattern": "(" }), json!({ "pattern": "x", "paths": ["nope"] })]
    {
        let resp = call(&mut server, "grep", arguments);
        assert_eq!(resp.result.as_ref().unwrap()["isError"], json!(true), "{resp:?}");
    }
}

#[test]
fn grep_annotates_symbols_when_a_graph_is_cached_and_runs_in_code_mode() {
    let repo = sample_repo();
    let mut server = McpServer::new(repo.path()).unwrap();
    let plain = payload(&call(&mut server, "grep", json!({ "pattern": "a \\* b" })));
    assert!(!plain["text"].as_str().unwrap().contains('['), "no graph, no annotation");
    assert!(!repo.path().join(".harness/.state/graph.bin").exists(), "grep must not build a graph");

    CodeGraphEngine::build(repo.path(), AnvilConfig::default()).unwrap().save().unwrap();
    let annotated = payload(&call(&mut server, "grep", json!({ "pattern": "a \\* b" })));
    assert!(
        annotated["text"].as_str().unwrap().contains("  6: return a * b;  [fn multiply]"),
        "{annotated}"
    );

    let scripted = payload(&call(
        &mut server,
        "code_exec",
        json!({ "script": "const r = anvil.grep({ pattern: 'add', paths: ['src'] }); return { n: r.total_matches, backend: r.backend };" }),
    ));
    assert_eq!(scripted["result"], json!({ "n": 3, "backend": "builtin" }));
}
