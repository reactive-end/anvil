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
fn post_tool_use_matches_namespaced_mutating_tool_and_stop_allows_non_idle_swarm() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    fs::write(root.join("src/bad_ns.ts"), "// TODO: fix\nexport const x: any = 1;\n").unwrap();
    let post = json!({
        "hookEventName": "PostToolUse",
        "cwd": root.to_string_lossy(),
        "toolCall": {
            "toolName": "default_api:edit_file",
            "toolInput": { "TargetFile": "src/bad_ns.ts" }
        }
    });
    assert_ok(
        &run_anvil_stdin(root, &["hook", "post-tool-use", "--adapter", "antigravity"], &post.to_string()),
        "namespaced edit_file post-tool-use",
    );
    let pre_inv = json!({ "hookEventName": "PreInvocation", "cwd": root.to_string_lossy() });
    let inv_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-invocation", "--adapter", "antigravity"], &pre_inv.to_string()),
        "drain namespaced diagnostic",
    );
    assert!(inv_out.contains("TODO"));

    let stop_non_idle = json!({
        "hookEventName": "Stop",
        "cwd": root.to_string_lossy(),
        "terminationReason": "TERMINAL_STEP_TYPE",
        "fullyIdle": false
    });
    let stop_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "stop", "--adapter", "antigravity"], &stop_non_idle.to_string()),
        "stop allows non-idle swarm turn",
    );
    assert_eq!(serde_json::from_str::<Value>(&stop_out).unwrap()["decision"], json!("allow"));
}

#[test]
fn doctor_resolves_node_modules_tsconfig_extends() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    fs::create_dir_all(root.join("node_modules/@tsconfig/node20")).unwrap();
    fs::write(
        root.join("node_modules/@tsconfig/node20/tsconfig.json"),
        r#"{"compilerOptions":{"strict":true}}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("node_modules/expo")).unwrap();
    fs::write(
        root.join("node_modules/expo/tsconfig.base.json"),
        r#"{"extends":"@tsconfig/node20"}"#,
    )
    .unwrap();
    fs::write(root.join("tsconfig.json"), r#"{"extends":"expo/tsconfig.base"}"#).unwrap();

    let doc_out = assert_ok(&run_anvil(root, &["doctor"]), "doctor node_modules extends");
    assert!(doc_out.contains("tsconfig: strict=true"));
    assert!(doc_out.contains("all critical checks passed"));
}

#[test]
fn check_full_scans_entire_repo_even_when_dirty() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    fs::write(root.join("src/unmodified_bad.ts"), "export const bad: any = 1;\n").unwrap();
    assert_ok(&run_anvil(root, &["init"]), "init");
    Command::new("git").args(["add", "-A"]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "initial"]).current_dir(root).output().unwrap();

    // Dirty only src/core.ts while src/unmodified_bad.ts stays clean in git.
    fs::write(root.join("src/core.ts"), "export function compute(x: number): number { return x + 1; }\n").unwrap();
    let full_out = run_anvil(root, &["check", "--full", "--tier", "0"]);
    assert!(!full_out.status.success(), "expected check --full to fail on unmodified_bad.ts");
    let stdout = String::from_utf8_lossy(&full_out.stdout);
    assert!(stdout.contains("unmodified_bad.ts"), "expected unmodified_bad.ts in output: {stdout}");
}

#[test]
fn impact_typecheck_exits_nonzero_and_qa_run_respects_phase_and_skip() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
    #[cfg(windows)]
    fs::write(
        root.join("node_modules/.bin/tsc.cmd"),
        "@echo off\r\necho src/core.ts(1,1): error TS2322: bad type\r\nexit /b 2\r\n",
    )
    .unwrap();
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = root.join("node_modules/.bin/tsc");
        fs::write(&p, "#!/bin/sh\necho 'src/core.ts(1,1): error TS2322: bad type'\nexit 2\n").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert_ok(&run_anvil(root, &["init"]), "init");

    let imp = run_anvil(root, &["impact", "src/core.ts", "--typecheck"]);
    assert!(!imp.status.success(), "expected impact --typecheck to exit nonzero");

    assert_ok(&run_anvil(root, &["qa", "select-web", "skip"]), "qa select-web skip");
    let qa_out = assert_ok(
        &run_anvil(root, &["qa", "run", "http://127.0.0.1:9999", "--phase", "before"]),
        "qa run --phase before with skip",
    );
    assert!(qa_out.contains("skipping `anvil qa run`"));
}

