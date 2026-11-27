//! `anvil search` and the PreToolUse search guard end to end in a temporary repository that has a
//! code graph and a docs bundle.

use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

const FORMATTERS: &str = "export function formatDate(d: Date): string {\n  return d.toISOString();\n}\n\nexport function formatMoney(n: number): string {\n  return n.toFixed(2);\n}\n\nexport function formatName(n: string): string {\n  return n.trim();\n}\n";
const CONCEPT: &str = "---\ntype: Code Module\ntitle: Invoicing\ndescription: How invoices are totalled\n---\n# Invoicing\n\nInvoices are totalled by the ledger service.\n";
const CONCEPT_PATH: &str = ".harness/docs/shop/invoicing/overview.md";
const HINT: &str =
    "Graph and docs were checked first; repeat the search if you still need the code.";

fn anvil(cwd: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_anvil"))
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env("ANVIL_NO_AUTO_DAEMON", "1")
        .env("ANVIL_LANG", "en")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn anvil");
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(text.as_bytes()).expect("stdin");
    }
    child.wait_with_output().expect("wait")
}

fn ok(cwd: &Path, args: &[&str]) -> String {
    let out = anvil(cwd, args, None);
    assert!(
        out.status.success(),
        "anvil {args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, text).expect("write");
}

/// A repository with a built graph and one documented concept.
fn repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write(root, "src/format.ts", FORMATTERS);
    write(root, "notes.md", "The zebra crossing is on the left.\n");
    write(root, CONCEPT_PATH, CONCEPT);
    ok(root, &["graph", "build"]);
    dir
}

fn hook(root: &Path, event: &str, payload: &Value) -> Value {
    let out = anvil(root, &["hook", event, "--adapter", "claude"], Some(&payload.to_string()));
    assert!(out.status.success(), "hook failed: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("hook JSON")
}

fn tool_call(root: &Path, tool: &str, tool_input: Value, agent: Option<&str>) -> Value {
    let mut payload = json!({
        "hook_event_name": "PreToolUse",
        "session_id": "sess-1",
        "cwd": root.to_string_lossy(),
        "tool_name": tool,
        "tool_input": tool_input,
    });
    if let Some(id) = agent {
        payload["agent_id"] = json!(id);
    }
    hook(root, "pre-tool-use", &payload)
}

fn new_prompt(root: &Path) {
    let payload = json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": "sess-1",
        "cwd": root.to_string_lossy(),
        "prompt": "next question",
    });
    hook(root, "user-prompt-submit", &payload);
}

fn decision(out: &Value) -> Option<&str> {
    out["hookSpecificOutput"]["permissionDecision"].as_str()
}

#[test]
fn search_prints_graph_then_docs_then_code_and_names_the_answering_stage() {
    let dir = repo();
    let root = dir.path();

    let graph = ok(root, &["search", "format"]);
    let (g, d, c) = (
        graph.find("## graph").unwrap(),
        graph.find("## docs").unwrap(),
        graph.find("## code").unwrap(),
    );
    assert!(g < d && d < c, "{graph}");
    assert!(graph.starts_with("# search \"format\": answered by graph"), "{graph}");
    assert!(graph.contains("symbol formatDate  src/format.ts:1"), "{graph}");
    assert!(graph.contains("## docs: no hits"), "{graph}");
    assert!(graph.contains("skipped: graph and docs answered"), "{graph}");

    let docs = ok(root, &["search", "invoices totalled"]);
    assert!(docs.contains("answered by docs"), "{docs}");
    assert!(docs.contains(&format!("shop/invoicing  {CONCEPT_PATH}")), "{docs}");
    assert!(docs.contains(&format!("next: `Read {CONCEPT_PATH}`")), "{docs}");

    let code = ok(root, &["search", "zebra"]);
    assert!(code.contains("answered by code"), "{code}");
    assert!(code.contains("notes.md\n  1: The zebra crossing is on the left."), "{code}");
}

#[test]
fn search_flags_force_the_code_stage_limit_hits_and_print_json() {
    let dir = repo();
    let root = dir.path();

    let forced = ok(root, &["search", "format", "--code", "--limit", "2"]);
    assert!(forced.contains("src/format.ts\n"), "code section present: {forced}");
    assert!(forced.contains("[fn formatDate]"), "code matches carry their symbol: {forced}");
    assert_eq!(
        forced.matches("\n  symbol ").count(),
        1,
        "limit 2 lists a file and a symbol: {forced}"
    );

    let literal = ok(root, &["search", "\"toISOString\""]);
    assert!(literal.contains("2: "), "quoted text greps the code: {literal}");

    let json: Value =
        serde_json::from_str(&ok(root, &["search", "zebra", "--json"])).expect("json");
    assert_eq!(json["answered_by"], "code");
    assert_eq!(json["code"]["report"]["total_matches"], 1);
    assert!(json["graph"]["hits"].as_array().unwrap().is_empty());

    assert!(!anvil(root, &["search", "  "], None).status.success());
}

#[test]
fn search_without_graph_or_docs_says_so_and_still_greps() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), "notes.md", "zebra\n");
    let text = ok(dir.path(), &["search", "zebra"]);
    assert!(text.contains("## graph: no code graph"), "{text}");
    assert!(text.contains("## docs: no docs bundle"), "{text}");
    assert!(text.contains("notes.md"), "{text}");
}

#[test]
fn the_hook_denies_the_first_grep_of_a_turn_then_allows_the_repeat() {
    let dir = repo();
    let root = dir.path();
    let grep = json!({ "pattern": "formatDate", "path": "src" });

    let first = tool_call(root, "Grep", grep.clone(), None);
    assert_eq!(decision(&first), Some("deny"), "{first}");
    let reason = first["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
    assert!(reason.contains("symbol formatDate  src/format.ts:1"), "{reason}");
    assert!(reason.ends_with(HINT), "{reason}");

    let second = tool_call(root, "Grep", grep.clone(), None);
    assert_eq!(decision(&second), None, "repeat passes: {second}");

    let sub = tool_call(root, "Grep", grep.clone(), Some("agent-7"));
    assert_eq!(decision(&sub), Some("deny"), "a subagent gets its own first search: {sub}");

    new_prompt(root);
    let after_prompt = tool_call(root, "Grep", grep, None);
    assert_eq!(
        decision(&after_prompt),
        Some("deny"),
        "a new prompt starts a new turn: {after_prompt}"
    );
}

#[test]
fn the_hook_lets_a_search_through_after_the_graph_or_a_shell_filter() {
    let dir = repo();
    let root = dir.path();

    let filter = tool_call(root, "Bash", json!({ "command": "cargo test | grep ok" }), None);
    assert_eq!(decision(&filter), None, "{filter}");
    let first = tool_call(root, "Bash", json!({ "command": "rg formatDate src" }), None);
    assert_eq!(decision(&first), Some("deny"), "{first}");

    new_prompt(root);
    let consult = tool_call(root, "mcp__anvil__graph_query", json!({ "term": "format" }), None);
    assert_eq!(decision(&consult), None, "{consult}");
    let after = tool_call(root, "Bash", json!({ "command": "rg formatDate src" }), None);
    assert_eq!(decision(&after), None, "graph consulted first: {after}");
}

#[test]
fn search_cascade_config_switches_the_guard_off() {
    let dir = repo();
    let root = dir.path();
    write(root, ".harness/config.json", r#"{"search":{"cascade":"off"}}"#);
    let out = tool_call(root, "Grep", json!({ "pattern": "formatDate" }), None);
    assert_eq!(decision(&out), None, "{out}");
}
