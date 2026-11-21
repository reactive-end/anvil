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
    Command::new("git").args(["config", "user.name", "Test User"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.email", "test@example.com"]).current_dir(root).output().unwrap();

    fs::write(root.join("package.json"), r#"{"name":"sample","version":"1.0.0"}"#).unwrap();
    fs::write(root.join("tsconfig.json"), r#"{"compilerOptions":{"strict":true}}"#).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/core.ts"), "export function compute(x: number): number {\n  return x * 2;\n}\n").unwrap();
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
fn circuit_breaker_open_allows_read_and_reset_while_blocking_edits() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    for _ in 0..3 {
        let fail_post = json!({
            "hook_event_name": "PostToolUse",
            "cwd": root.to_string_lossy(),
            "tool_name": "Bash",
            "tool_input": { "command": "tsc --noEmit" },
            "tool_output": { "exitCode": 2, "stderr": "TS2322 type mismatch" }
        });
        assert_ok(
            &run_anvil_stdin(root, &["hook", "post-tool-use", "--adapter", "claude"], &fail_post.to_string()),
            "trip circuit breaker",
        );
    }

    let read_pre = json!({
        "hook_event_name": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "tool_name": "Read",
        "tool_input": { "file_path": "src/core.ts" }
    });
    let read_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-tool-use", "--adapter", "claude"], &read_pre.to_string()),
        "pre-tool-use Read",
    );
    let read_json: Value = serde_json::from_str(&read_out).unwrap();
    assert_eq!(read_json["hookSpecificOutput"]["permissionDecision"], json!("allow"));

    let reset_pre = json!({
        "hook_event_name": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "tool_name": "Bash",
        "tool_input": { "command": "anvil circuit reset" }
    });
    let reset_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-tool-use", "--adapter", "claude"], &reset_pre.to_string()),
        "pre-tool-use circuit reset",
    );
    let reset_json: Value = serde_json::from_str(&reset_out).unwrap();
    assert_eq!(reset_json["hookSpecificOutput"]["permissionDecision"], json!("allow"));

    let edit_pre = json!({
        "hook_event_name": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "tool_name": "edit_file",
        "tool_input": { "TargetFile": "src/core.ts" }
    });
    let edit_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-tool-use", "--adapter", "claude"], &edit_pre.to_string()),
        "pre-tool-use edit_file",
    );
    let edit_json: Value = serde_json::from_str(&edit_out).unwrap();
    assert_eq!(edit_json["hookSpecificOutput"]["permissionDecision"], json!("deny"));
}
