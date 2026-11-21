use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

fn anvil_bin() -> &'static str {
    env!("CARGO_BIN_EXE_anvil")
}

fn run_anvil(repo: &Path, args: &[&str]) -> Output {
    Command::new(anvil_bin())
        .arg("--root")
        .arg(repo)
        .args(args)
        .output()
        .expect("failed to execute anvil binary")
}

fn run_anvil_stdin(repo: &Path, args: &[&str], stdin_data: &str) -> Output {
    let mut child = Command::new(anvil_bin())
        .arg("--root")
        .arg(repo)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn anvil binary");

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(stdin_data.as_bytes()).unwrap();
    }
    child.wait_with_output().unwrap()
}

fn setup_git_ts_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    Command::new("git").args(["init"]).current_dir(root).output().unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(root)
        .output()
        .unwrap();

    fs::write(root.join("package.json"), r#"{"name":"sample","version":"1.0.0"}"#).unwrap();
    fs::write(
        root.join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true}}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/core.ts"),
        "export function compute(x: number): number {\n  return x * 2;\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("src/page.ts"),
        "import { compute } from './core';\nexport function renderPage(): number {\n  return compute(21);\n}\n",
    )
    .unwrap();
    dir
}

fn assert_ok(out: &Output, label: &str) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "{label} failed ({:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code()
    );
    stdout
}

#[test]
fn cli_init_doctor_graph_and_context_pack() {
    let repo = setup_git_ts_repo();
    let root = repo.path();

    let init_out = assert_ok(&run_anvil(root, &["init"]), "init");
    assert!(init_out.contains("anvil initialized"));
    assert!(root.join(".harness/config.json").exists());
    assert!(root.join(".agents/hooks.json").exists());
    assert!(root.join(".claude/settings.json").exists());
    assert!(root.join("tasks.json").exists());

    let doc_out = assert_ok(&run_anvil(root, &["doctor"]), "doctor");
    assert!(doc_out.contains("all critical checks passed"));

    let build_out = assert_ok(&run_anvil(root, &["graph", "build"]), "graph build");
    assert!(build_out.contains("graph built:"));

    let query_out = assert_ok(&run_anvil(root, &["graph", "query", "compute"]), "graph query");
    assert!(query_out.contains("compute"));

    let impact_out = assert_ok(
        &run_anvil(root, &["impact", "src/core.ts", "--depth", "2"]),
        "impact",
    );
    assert!(impact_out.contains("src/page.ts"));

    let ctx_out = assert_ok(
        &run_anvil(root, &["context", "pack", "src/core.ts", "--max-tokens", "400"]),
        "context pack",
    );
    assert!(ctx_out.contains("Minimal Context Packet"));
    assert!(ctx_out.contains("src/core.ts"));
}

#[test]
fn cli_memory_tasks_and_circuit_lifecycle() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    let save_out = assert_ok(
        &run_anvil(
            root,
            &[
                "mem", "save", "--kind", "decision", "--title", "Strict TS",
                "--what", "Enable strict mode", "--why", "Catch null bugs early",
                "--where", "tsconfig.json", "--learned", "Always keep strict true",
            ],
        ),
        "mem save",
    );
    let obs_id = save_out
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix("id="))
        .unwrap()
        .to_owned();

    let search_out = assert_ok(&run_anvil(root, &["mem", "search", "Strict TS"]), "mem search");
    assert!(search_out.contains(&obs_id));
    let get_out = assert_ok(&run_anvil(root, &["mem", "get", &obs_id]), "mem get");
    assert!(get_out.contains("Always keep strict true"));

    let start_out = assert_ok(&run_anvil(root, &["tasks", "start", "Implement feature X"]), "tasks start");
    let task_id = start_out
        .split_whitespace()
        .find(|tok| tok.starts_with("T-"))
        .unwrap()
        .to_owned();
    assert_ok(&run_anvil(root, &["tasks", "update", &task_id, "Added core logic"]), "tasks update");
    assert_ok(&run_anvil(root, &["tasks", "done", &task_id, "Verified tests"]), "tasks done");
    let build_out = assert_ok(&run_anvil(root, &["tasks", "build"]), "tasks build");
    assert!(build_out.contains("1 task(s)"));

    let st_out = assert_ok(&run_anvil(root, &["circuit", "status"]), "circuit status");
    assert!(st_out.contains("Closed"));
    let rst_out = assert_ok(&run_anvil(root, &["circuit", "reset"]), "circuit reset");
    assert!(rst_out.contains("circuit reset: Closed"));
}

#[test]
fn cli_hooks_antigravity_and_claude_roundtrip() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    // Antigravity PostToolUse queues into inbox.json and returns {}; PreInvocation drains injectSteps.
    let agy_post = json!({
        "hookEventName": "PostToolUse",
        "cwd": root.to_string_lossy(),
        "toolCall": {
            "toolName": "replace_file_content",
            "toolInput": { "TargetFile": "src/bad.ts" }
        }
    });
    fs::write(root.join("src/bad.ts"), "// TODO: remove later\nexport const x: any = 1;\n").unwrap();
    let post_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "post-tool-use", "--adapter", "antigravity"], &agy_post.to_string()),
        "agy post-tool-use",
    );
    assert_eq!(serde_json::from_str::<Value>(&post_out).unwrap(), json!({}));

    let pre_inv = json!({ "hookEventName": "PreInvocation", "cwd": root.to_string_lossy() });
    let inv_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-invocation", "--adapter", "antigravity"], &pre_inv.to_string()),
        "agy pre-invocation",
    );
    let inv_json: Value = serde_json::from_str(&inv_out).unwrap();
    assert!(inv_json["injectSteps"][0]["systemMessage"]["systemMessage"].as_str().unwrap().contains("TODO"));

    // Claude PostToolUse returns additionalContext immediately; Stop honors continuationActive loop guard.
    let claude_post = json!({
        "hook_event_name": "PostToolUse",
        "cwd": root.to_string_lossy(),
        "tool_name": "Edit",
        "tool_input": { "file_path": "src/bad.ts" }
    });
    let claude_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "post-tool-use", "--adapter", "claude"], &claude_post.to_string()),
        "claude post-tool-use",
    );
    let claude_json: Value = serde_json::from_str(&claude_out).unwrap();
    assert!(claude_json["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("TODO"));

    let stop_guard = json!({ "hookEventName": "Stop", "continuationActive": true });
    let stop_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "stop", "--adapter", "antigravity"], &stop_guard.to_string()),
        "agy stop continuation guard",
    );
    assert_eq!(serde_json::from_str::<Value>(&stop_out).unwrap()["decision"], json!("allow"));
}
