use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::io::Write;

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

    // Trip Circuit Breaker OPEN with 3 identical failures via PostToolUse.
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

    // 1. Read tool MUST be allowed even when OPEN.
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

    // 2. `anvil circuit reset` MUST be allowed even when OPEN.
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

    // 3. Mutating edit_file MUST be blocked while OPEN.
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

#[test]
fn post_tool_use_read_skips_ast_and_memory_invalidation() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    // Put a Tier 0 violation in src/bad.ts and link a memory observation to it.
    fs::write(root.join("src/bad.ts"), "// TODO: fix me\nexport const v: any = 1;\n").unwrap();
    let save_out = assert_ok(
        &run_anvil(
            root,
            &[
                "mem", "save", "--kind", "decision", "--title", "Bad file note",
                "--what", "Track bad.ts", "--why", "Test read hook isolation",
                "--where", "src/bad.ts", "--learned", "Reads must not stale memory",
                "--files", "src/bad.ts",
            ],
        ),
        "mem save",
    );
    let obs_id = save_out.split_whitespace().find_map(|t| t.strip_prefix("id=")).unwrap().to_owned();

    // Fire PostToolUse for Read -> must emit {} and keep memory Fresh.
    let read_post = json!({
        "hook_event_name": "PostToolUse",
        "cwd": root.to_string_lossy(),
        "tool_name": "Read",
        "tool_input": { "file_path": "src/bad.ts" }
    });
    let post_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "post-tool-use", "--adapter", "claude"], &read_post.to_string()),
        "claude post-tool-use Read",
    );
    assert_eq!(serde_json::from_str::<Value>(&post_out).unwrap(), json!({}));

    let get_out = assert_ok(&run_anvil(root, &["mem", "get", &obs_id]), "mem get after Read");
    let obs_json: Value = serde_json::from_str(&get_out).unwrap();
    assert_eq!(obs_json["freshness"], json!("fresh"));
}

#[test]
fn ratchet_test_and_promoted_rule_live_in_anvil_check() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");
    Command::new("git").args(["add", "-A"]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "initial"]).current_dir(root).output().unwrap();

    // Log candidate, test it with `anvil ratchet test`, review on clean sample, and promote.
    let log_out = assert_ok(
        &run_anvil(
            root,
            &[
                "ratchet", "log",
                "--rule", "no-console-log",
                "--description", "Avoid console.log in production TypeScript",
                "--pattern", r"console\.log\(",
                "--lang", "ts",
                "--high-impact",
            ],
        ),
        "ratchet log",
    );
    let cand_id = log_out.split_whitespace().find_map(|t| t.strip_prefix("id=")).unwrap().to_owned();

    let test_out = assert_ok(&run_anvil(root, &["ratchet", "test", &cand_id]), "ratchet test");
    assert!(test_out.contains("scanned=1"));
    assert!(test_out.contains("matches=0"));

    assert_ok(
        &run_anvil(root, &["ratchet", "review", &cand_id, "--samples", "export const ok = 1;"]),
        "ratchet review",
    );
    assert_ok(&run_anvil(root, &["ratchet", "promote", &cand_id]), "ratchet promote");

    // Modify a TS file to violate the promoted rule and run `anvil check`.
    fs::write(root.join("src/core.ts"), "export function compute(x: number): number {\n  console.log(x);\n  return x;\n}\n").unwrap();
    let check_out = assert_ok(&run_anvil(root, &["check", "--tier", "0"]), "anvil check");
    assert!(check_out.contains("no-console-log"));
}

#[test]
fn graph_update_prints_nonzero_added_count_on_new_file() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    fs::write(root.join("src/added.ts"), "export const extra = 42;\n").unwrap();
    let update_out = assert_ok(&run_anvil(root, &["graph", "update"]), "graph update");
    assert!(
        update_out.contains("+1 added"),
        "expected `+1 added` in graph update output, got: {update_out}"
    );
}

#[test]
fn doctor_passes_on_jsonc_tsconfig_with_extends() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    fs::write(
        root.join("tsconfig.base.json"),
        "// base strict config\n{\n  \"compilerOptions\": {\n    \"strict\": true,\n  },\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("tsconfig.json"),
        "/* extends base with trailing comma */\n{\n  \"extends\": \"./tsconfig.base.json\",\n  \"compilerOptions\": {\n    \"target\": \"ES2022\",\n  },\n}\n",
    )
    .unwrap();

    let doc_out = assert_ok(&run_anvil(root, &["doctor"]), "doctor jsonc extends");
    assert!(doc_out.contains("tsconfig: strict=true"));
    assert!(doc_out.contains("all critical checks passed"));
}

#[test]
fn hook_pre_tool_use_skips_wrapping_commands_with_shell_metacharacters() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    let piped = json!({
        "hook_event_name": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "tool_name": "Bash",
        "tool_input": { "command": "tsc --noEmit && echo done" }
    });
    let out = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-tool-use", "--adapter", "claude"], &piped.to_string()),
        "pre-tool-use metachar",
    );
    let parsed: Value = serde_json::from_str(&out).unwrap();
    assert!(parsed["hookSpecificOutput"]["updatedInput"].is_null());
}

#[test]
fn mem_sync_prints_nonzero_ingested_count_for_new_chunk() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    let chunks_dir = root.join(".harness/memory/chunks");
    fs::create_dir_all(&chunks_dir).unwrap();
    let chunk = json!({
        "id": "01J00000000000000000000001",
        "topic_key": "decision/sync-test",
        "kind": "decision",
        "title": "Sync test chunk",
        "what": "Verify mem sync ingests new chunks on first call",
        "why": "Avoid double sync eating ingested count",
        "location": "commands/mem.rs",
        "learned": "Open without sync before mem.sync()",
        "related_files": ["src/core.ts"],
        "related_symbols": [],
        "author": "teammate",
        "created_at": 1700000000000i64,
        "updated_at": 1700000000000i64,
        "expires_at": null
    });
    fs::write(chunks_dir.join("teammate-01J00000000000000000000001.jsonl"), format!("{chunk}\n")).unwrap();

    let sync_out = assert_ok(&run_anvil(root, &["mem", "sync"]), "mem sync");
    assert!(
        sync_out.contains("ingested=1") && sync_out.contains("applied=1"),
        "expected ingested=1 applied=1, got: {sync_out}"
    );
}

