//! E2E tests for security guard, `anvil run` hook wrapping, `--git-hooks`, `--staged`,
//! architecture boundaries/cycles, and zero-noise read-only `Stop` turns.

use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

fn anvil_bin() -> &'static str {
    env!("CARGO_BIN_EXE_anvil")
}

fn run_anvil(cwd: &Path, args: &[&str]) -> Output {
    Command::new(anvil_bin())
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env("ANVIL_NO_AUTO_DAEMON", "1")
        .output()
        .expect("run anvil")
}

fn run_anvil_stdin(cwd: &Path, args: &[&str], stdin_data: &str) -> Output {
    let mut child = Command::new(anvil_bin())
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env("ANVIL_NO_AUTO_DAEMON", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn anvil");
    child.stdin.as_mut().unwrap().write_all(stdin_data.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

fn assert_ok(out: &Output, label: &str) -> String {
    assert!(
        out.status.success(),
        "{label} failed ({:?}):\nstdout={}\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn setup_git_ts_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    Command::new("git").args(["init"]).current_dir(root).output().unwrap();
    Command::new("git")
        .args(["config", "user.email", "dev@example.com"])
        .current_dir(root)
        .output()
        .unwrap();
    Command::new("git").args(["config", "user.name", "Dev"]).current_dir(root).output().unwrap();
    fs::write(root.join("package.json"), r#"{"name":"phase4-demo","version":"1.0.0"}"#).unwrap();
    fs::write(
        root.join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true,"target":"ES2022"}}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.ts"), "export const a = 1;\n").unwrap();
    Command::new("git").args(["add", "-A"]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "init"]).current_dir(root).output().unwrap();
    dir
}

fn assert_env_reads_prompt_only_for_real_secrets(root: &Path) {
    let secret_read = json!({
        "hook_event_name": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "tool_name": "Read",
        "tool_input": { "file_path": ".env" }
    });
    let secret_out: Value = serde_json::from_str(&assert_ok(
        &run_anvil_stdin(
            root,
            &["hook", "pre-tool-use", "--adapter", "claude"],
            &secret_read.to_string(),
        ),
        "secret read",
    ))
    .unwrap();
    assert_eq!(secret_out["hookSpecificOutput"]["permissionDecision"], json!("ask"));

    let safe_env = json!({
        "hook_event_name": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "tool_name": "Read",
        "tool_input": { "file_path": ".env.example" }
    });
    let safe_out: Value = serde_json::from_str(&assert_ok(
        &run_anvil_stdin(
            root,
            &["hook", "pre-tool-use", "--adapter", "claude"],
            &safe_env.to_string(),
        ),
        "safe env template",
    ))
    .unwrap();
    assert_ne!(safe_out["hookSpecificOutput"]["permissionDecision"], json!("ask"));
}

#[test]
fn security_guard_prompts_on_destructive_cmds_and_secrets_and_blocks_godot_generated() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    let destructive = json!({
        "hook_event_name": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "tool_name": "Bash",
        "tool_input": { "command": "git reset --hard HEAD" }
    });
    let out: Value = serde_json::from_str(&assert_ok(
        &run_anvil_stdin(
            root,
            &["hook", "pre-tool-use", "--adapter", "claude"],
            &destructive.to_string(),
        ),
        "destructive cmd",
    ))
    .unwrap();
    assert_eq!(out["hookSpecificOutput"]["permissionDecision"], json!("ask"));

    assert_env_reads_prompt_only_for_real_secrets(root);

    let godot_meta = json!({
        "hookEventName": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "toolCall": { "toolName": "write_to_file", "toolInput": { "TargetFile": "icon.png.import", "CodeContent": "" } }
    });
    let godot_out: Value = serde_json::from_str(&assert_ok(
        &run_anvil_stdin(
            root,
            &["hook", "pre-tool-use", "--adapter", "antigravity"],
            &godot_meta.to_string(),
        ),
        "godot generated deny",
    ))
    .unwrap();
    assert_eq!(godot_out["decision"], json!("deny"));
}

#[test]
fn pre_tool_use_wraps_cargo_and_godot_commands_with_anvil_run() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    for cmd in ["cargo test --workspace", "godot --headless --quit"] {
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "cwd": root.to_string_lossy(),
            "tool_name": "Bash",
            "tool_input": { "command": cmd }
        });
        let out: Value = serde_json::from_str(&assert_ok(
            &run_anvil_stdin(
                root,
                &["hook", "pre-tool-use", "--adapter", "claude"],
                &payload.to_string(),
            ),
            "wrap command",
        ))
        .unwrap();
        assert_eq!(
            out["hookSpecificOutput"]["updatedInput"]["command"],
            json!(format!("anvil run -- {cmd}"))
        );
    }
}

#[test]
fn init_git_hooks_and_check_staged_only_audits_staged_files() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init", "--git-hooks"]), "init --git-hooks");

    let hook_path = root.join(".git/hooks/pre-commit");
    assert!(hook_path.is_file());
    let hook_text = fs::read_to_string(&hook_path).unwrap();
    assert!(hook_text.contains("anvil check --staged --tier 0"));

    // Unstaged broken file vs staged clean file.
    fs::write(
        root.join("src/unstaged_bad.ts"),
        concat!("// @", "ts-ignore\nexport const bad = 1;\n"),
    )
    .unwrap();
    fs::write(root.join("src/staged_ok.ts"), "export const ok = 42;\n").unwrap();
    Command::new("git").args(["add", "src/staged_ok.ts"]).current_dir(root).output().unwrap();

    assert_ok(
        &run_anvil(root, &["check", "--staged", "--tier", "0"]),
        "check --staged ignores unstaged file",
    );
    let full_wd = run_anvil(root, &["check", "--tier", "0"]);
    assert!(!full_wd.status.success(), "working-tree check must fail on unstaged @ts-ignore");
}

#[test]
fn check_enforces_configured_boundaries_and_import_cycles() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    fs::create_dir_all(root.join("src/ui")).unwrap();
    fs::create_dir_all(root.join("src/db")).unwrap();
    fs::write(root.join("src/db/repo.ts"), "export const query = () => 1;\n").unwrap();
    fs::write(
        root.join("src/ui/view.ts"),
        "import { query } from '../db/repo';\nexport const v = query();\n",
    )
    .unwrap();
    assert_ok(&run_anvil(root, &["init"]), "init");

    let cfg_path = root.join(".harness/config.json");
    let mut cfg: Value = serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg["boundaries"] = json!([{
        "name": "ui-no-db",
        "from_glob": "src/ui/**",
        "forbid_glob": "src/db/**",
        "reason": "UI must not import DB directly"
    }]);
    cfg["forbid_import_cycles"] = json!(true);
    fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();

    let failed = run_anvil(root, &["check", "--tier", "1", "--no-tests", "--no-cache"]);
    assert!(!failed.status.success(), "boundary violation must fail check");
    let stdout = String::from_utf8_lossy(&failed.stdout);
    assert!(stdout.contains("boundary:ui-no-db"), "expected boundary diagnostic: {stdout}");
}

#[test]
fn stop_hook_emits_zero_inbox_noise_on_readonly_turns() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    // Turn 1: real edit -> Stop runs and stamps last-stop.
    fs::write(root.join("src/a.ts"), "export const a = 2;\n").unwrap();
    let edit = json!({
        "hookEventName": "PostToolUse",
        "cwd": root.to_string_lossy(),
        "toolCall": { "toolName": "write_to_file", "toolInput": { "TargetFile": "src/a.ts" } }
    });
    assert_ok(
        &run_anvil_stdin(
            root,
            &["hook", "post-tool-use", "--adapter", "antigravity"],
            &edit.to_string(),
        ),
        "post-tool",
    );
    let stop_in = json!({ "hookEventName": "Stop", "cwd": root.to_string_lossy() }).to_string();
    assert_ok(
        &run_anvil_stdin(root, &["hook", "stop", "--adapter", "antigravity"], &stop_in),
        "turn1 stop",
    );

    // Drain any Turn 1 inbox messages.
    let pre_inv = json!({ "hookEventName": "PreInvocation", "cwd": root.to_string_lossy(), "lastUserInput": "How does a.ts work?" }).to_string();
    let _ = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-invocation", "--adapter", "antigravity"], &pre_inv),
        "turn2 pre-inv",
    );

    // Turn 2: strictly read-only, then Stop -> zero messages added to inbox.json.
    assert_ok(
        &run_anvil_stdin(root, &["hook", "stop", "--adapter", "antigravity"], &stop_in),
        "turn2 readonly stop",
    );
    let pre_inv_after = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-invocation", "--adapter", "antigravity"], &pre_inv),
        "turn3 drain",
    );
    assert!(
        !pre_inv_after.contains("tier1:impact"),
        "readonly Stop must not emit blast-radius noise: {pre_inv_after}"
    );
    assert!(
        !pre_inv_after.contains("tier2:qa"),
        "readonly Stop must not emit QA noise: {pre_inv_after}"
    );
}

#[test]
fn uninitialized_root_hooks_never_pollute_state_and_umbrella_sync_stays_clean() {
    let ws = TempDir::new().unwrap();
    let root = ws.path();
    fs::create_dir_all(root.join("svc-a/src")).unwrap();
    fs::write(root.join("svc-a/package.json"), r#"{"name":"svc-a"}"#).unwrap();
    fs::write(root.join("svc-a/src/index.ts"), "export const x = 1;\n").unwrap();

    assert_ok(&run_anvil(root, &["init", "--layout", "multi-repo"]), "init umbrella");
    assert!(!root.join(".harness").exists(), "umbrella init must not create root .harness/");

    assert_ok(&run_anvil(root, &["sync"]), "umbrella sync");
    assert!(!root.join(".harness").exists(), "umbrella sync must not create root .harness/");

    let post_root = json!({
        "hookEventName": "PostToolUse",
        "cwd": root.to_string_lossy(),
        "toolCall": { "toolName": "Bash", "toolInput": { "command": "echo ok" }, "toolOutput": { "exitCode": 0 } }
    });
    assert_ok(
        &run_anvil_stdin(
            root,
            &["hook", "post-tool-use", "--adapter", "antigravity"],
            &post_root.to_string(),
        ),
        "post at umbrella root",
    );
    assert!(
        !root.join(".harness").exists(),
        "PostToolUse at uninitialized root must not create .harness/"
    );
}
