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
    fs::write(root.join("tsconfig.json"), r#"{"compilerOptions":{"strict":true}}"#).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/core.ts"),
        "export function compute(x: number): number {\n  return x * 2;\n}\n",
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

fn pre_tool_decision(root: &Path, tool_name: &str, tool_input: Value) -> (Value, String) {
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "tool_name": tool_name,
        "tool_input": tool_input,
    });
    let out = assert_ok(
        &run_anvil_stdin(
            root,
            &["hook", "pre-tool-use", "--adapter", "claude"],
            &payload.to_string(),
        ),
        tool_name,
    );
    let parsed: Value = serde_json::from_str(&out).unwrap();
    if parsed == json!({}) {
        return (json!("none"), String::new());
    }
    assert!(parsed["hookSpecificOutput"]["permissionDecision"].is_string());
    (
        parsed["hookSpecificOutput"]["permissionDecision"].clone(),
        parsed["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap_or("").to_owned(),
    )
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
            &run_anvil_stdin(
                root,
                &["hook", "post-tool-use", "--adapter", "claude"],
                &fail_post.to_string(),
            ),
            "trip circuit breaker",
        );
    }

    assert_eq!(
        pre_tool_decision(root, "Read", json!({ "file_path": "src/core.ts" })).0,
        json!("none")
    );
    assert_eq!(
        pre_tool_decision(root, "Bash", json!({ "command": "anvil circuit status" })).0,
        json!("none")
    );
    let (reset_dec, reset_reason) =
        pre_tool_decision(root, "Bash", json!({ "command": "anvil circuit reset" }));
    assert_eq!(reset_dec, json!("ask"));
    assert!(reset_reason.contains("confirm before running `anvil circuit reset`"));
    let (edit_dec, edit_reason) =
        pre_tool_decision(root, "edit_file", json!({ "TargetFile": "src/core.ts" }));
    assert_eq!(edit_dec, json!("deny"));
    assert!(edit_reason.contains("Circuit Breaker OPEN"));
}

#[test]
fn empty_stderr_with_distinct_stdout_does_not_false_trip_circuit_breaker() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    for msg in ["first fail A", "second fail B", "third fail C"] {
        let payload = json!({
            "hook_event_name": "PostToolUse",
            "cwd": root.to_string_lossy(),
            "tool_name": "Bash",
            "tool_input": { "command": "custom-test" },
            "tool_output": { "exitCode": 1, "stderr": "", "stdout": msg }
        });
        assert_ok(
            &run_anvil_stdin(
                root,
                &["hook", "post-tool-use", "--adapter", "claude"],
                &payload.to_string(),
            ),
            "post fail with stdout",
        );
    }

    let (edit_dec, _) =
        pre_tool_decision(root, "edit_file", json!({ "TargetFile": "src/core.ts" }));
    assert_eq!(edit_dec, json!("none"));
}
