use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::json;
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
fn anvil_run_times_out_slow_command_with_exit_124() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    let mut cmd = Command::new(anvil_bin());
    cmd.arg("--root")
        .arg(root)
        .env("ANVIL_RUN_TIMEOUT_SECS", "1")
        .arg("run")
        .arg("--");

    #[cfg(windows)]
    cmd.args(["powershell", "-NoProfile", "-Command", "Start-Sleep -Seconds 10"]);
    #[cfg(not(windows))]
    cmd.args(["sh", "-c", "sleep 10"]);

    let out = cmd.output().expect("failed to execute anvil run");
    assert_eq!(out.status.code(), Some(124));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("FAIL"), "expected FAIL report: {stdout}");
    assert!(stdout.contains("exit=124"), "expected exit=124 in report: {stdout}");
}

#[test]
fn post_tool_use_git_status_does_not_reset_cargo_test_streak() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    for _ in 0..2 {
        let fail_post = json!({
            "hook_event_name": "PostToolUse",
            "cwd": root.to_string_lossy(),
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test" },
            "tool_output": { "exitCode": 101, "stderr": "error[E0308]: mismatched types" }
        });
        assert_ok(
            &run_anvil_stdin(root, &["hook", "post-tool-use", "--adapter", "claude"], &fail_post.to_string()),
            "cargo test fail",
        );
    }

    // Passing `git status` must NOT wipe the `cargo test` failure streak.
    let git_ok_post = json!({
        "hook_event_name": "PostToolUse",
        "cwd": root.to_string_lossy(),
        "tool_name": "Bash",
        "tool_input": { "command": "git status" },
        "tool_output": { "exitCode": 0, "output": "nothing to commit" }
    });
    assert_ok(
        &run_anvil_stdin(root, &["hook", "post-tool-use", "--adapter", "claude"], &git_ok_post.to_string()),
        "git status ok",
    );

    let third_fail = json!({
        "hook_event_name": "PostToolUse",
        "cwd": root.to_string_lossy(),
        "tool_name": "Bash",
        "tool_input": { "command": "cargo test" },
        "tool_output": { "exitCode": 101, "stderr": "error[E0308]: mismatched types" }
    });
    assert_ok(
        &run_anvil_stdin(root, &["hook", "post-tool-use", "--adapter", "claude"], &third_fail.to_string()),
        "third cargo test fail",
    );

    let circuit_out = assert_ok(&run_anvil(root, &["circuit", "status"]), "circuit status");
    assert!(
        circuit_out.contains("circuit: Open") && circuit_out.contains("consecutive_failures=3/3"),
        "expected circuit breaker to trip Open after 3rd cargo test failure: {circuit_out}"
    );
}

#[test]
fn post_tool_use_tier0_ast_failures_trip_and_clean_edit_resets_streak() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    // Two edits with a Tier 0 error increment the tier0-ast failure streak.
    fs::write(root.join("src/core.ts"), "// @ts-ignore\nexport const v = 1;\n").unwrap();
    for _ in 0..2 {
        let edit_post = json!({
            "hook_event_name": "PostToolUse",
            "cwd": root.to_string_lossy(),
            "tool_name": "Edit",
            "tool_input": { "file_path": "src/core.ts" }
        });
        assert_ok(
            &run_anvil_stdin(root, &["hook", "post-tool-use", "--adapter", "claude"], &edit_post.to_string()),
            "tier0 fail edit",
        );
    }

    let st_mid = assert_ok(&run_anvil(root, &["circuit", "status"]), "circuit status mid");
    assert!(
        st_mid.contains("consecutive_failures=2/3"),
        "expected tier0-ast streak=2/3: {st_mid}"
    );

    // Clean edit clears the tier0-ast streak before N=3.
    fs::write(root.join("src/core.ts"), "export const v: number = 1;\n").unwrap();
    let clean_post = json!({
        "hook_event_name": "PostToolUse",
        "cwd": root.to_string_lossy(),
        "tool_name": "Edit",
        "tool_input": { "file_path": "src/core.ts" }
    });
    assert_ok(
        &run_anvil_stdin(root, &["hook", "post-tool-use", "--adapter", "claude"], &clean_post.to_string()),
        "tier0 clean edit",
    );

    let st_after = assert_ok(&run_anvil(root, &["circuit", "status"]), "circuit status after clean");
    assert!(
        st_after.contains("consecutive_failures=0/3"),
        "expected clean edit to reset tier0-ast streak: {st_after}"
    );

    // Three consecutive Tier 0 errors trip Circuit Breaker OPEN.
    fs::write(root.join("src/core.ts"), "// @ts-ignore\nexport const v = 1;\n").unwrap();
    for _ in 0..3 {
        let edit_post = json!({
            "hook_event_name": "PostToolUse",
            "cwd": root.to_string_lossy(),
            "tool_name": "Edit",
            "tool_input": { "file_path": "src/core.ts" }
        });
        assert_ok(
            &run_anvil_stdin(root, &["hook", "post-tool-use", "--adapter", "claude"], &edit_post.to_string()),
            "tier0 trip edit",
        );
    }
    let st_open = assert_ok(&run_anvil(root, &["circuit", "status"]), "circuit status open");
    assert!(
        st_open.contains("circuit: Open"),
        "expected tier0-ast to trip circuit Open: {st_open}"
    );
}

#[test]
fn doctor_checks_cursor_windsurf_and_codex_adapters() {
    for (flag, files) in [
        ("cursor", vec![".cursor/mcp.json", ".cursor/rules/anvil.mdc"]),
        ("windsurf", vec![".windsurf/mcp_config.json", ".windsurf/rules/anvil.md"]),
        ("codex", vec![".codex/config.toml", "AGENTS.md"]),
    ] {
        let repo = setup_git_ts_repo();
        let root = repo.path();
        assert_ok(&run_anvil(root, &["init", "--adapter", flag]), &format!("init {flag}"));
        assert_ok(&run_anvil(root, &["doctor"]), &format!("doctor {flag} ok"));

        fs::remove_file(root.join(files[0])).unwrap();
        let fail_doc = run_anvil(root, &["doctor"]);
        assert!(
            !fail_doc.status.success(),
            "expected doctor to fail when {} is missing for adapter {flag}",
            files[0]
        );
    }
}
