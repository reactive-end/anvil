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
fn anvil_run_plus_post_tool_use_does_not_flip_flop_circuit_streak() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    for attempt in 0..3 {
        let cmd_str = "rustc nonexistent_file_12345.rs";
        let run_out = Command::new(anvil_bin())
            .arg("--root")
            .arg(root)
            .args(["run", "--", "rustc", "nonexistent_file_12345.rs"])
            .output()
            .expect("anvil run");
        assert!(!run_out.status.success(), "expected wrapped command to fail on attempt {attempt}");

        let wrapped_post = json!({
            "hook_event_name": "PostToolUse",
            "cwd": root.to_string_lossy(),
            "tool_name": "Bash",
            "tool_input": { "command": format!("anvil run -- {cmd_str}") },
            "tool_output": {
                "exitCode": run_out.status.code().unwrap_or(1),
                "output": String::from_utf8_lossy(&run_out.stdout).to_string()
            }
        });
        assert_ok(
            &run_anvil_stdin(root, &["hook", "post-tool-use", "--adapter", "claude"], &wrapped_post.to_string()),
            "wrapped post-tool-use",
        );
    }

    let st = assert_ok(&run_anvil(root, &["circuit", "status"]), "circuit status after 3 wrapped runs");
    assert!(
        st.contains("circuit: Open") && st.contains("consecutive_failures=3/3"),
        "wrapped `anvil run` + PostToolUse must reach Open (no flip-flop reset): {st}"
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

#[test]
fn orchestrator_setup_generates_agents_and_toggles_default_agent() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");
    assert!(!root.join(".claude/agents/anvil-executor.md").exists(), "agents are opt-in");

    for bad in [["--model", "opus\ntools: Bash"], ["--effort", "ultra"], ["--executor-model", "gpt-5"]] {
        let out = run_anvil(root, &["orchestrator", "setup", bad[0], bad[1]]);
        assert!(!out.status.success(), "{bad:?} must be rejected");
    }
    let setup = ["orchestrator", "setup", "--model", "claude-opus-5-5", "--effort", "xhigh", "--executor-model", "haiku", "--executor-effort", "low"];
    assert_ok(&run_anvil(root, &setup), "setup");
    let orchestrator = fs::read_to_string(root.join(".claude/agents/anvil-orchestrator.md")).unwrap();
    assert!(orchestrator.starts_with("---\nname: anvil-orchestrator\n"));
    assert!(orchestrator.contains("model: claude-opus-5-5\neffort: xhigh\ndisallowedTools: Edit, Write, NotebookEdit\n"));
    let executor_path = root.join(".claude/agents/anvil-executor.md");
    assert!(fs::read_to_string(&executor_path).unwrap().contains("model: haiku\neffort: low\n"));
    assert!(fs::read_to_string(&executor_path).unwrap().contains(
        "tools: Read, Edit, Write, NotebookEdit, Grep, Glob, Bash, PowerShell, WebFetch, WebSearch, mcp__anvil__*\n"
    ));
    assert!(root.join(".claude/skills/orchestrate/SKILL.md").is_file());

    assert_ok(&run_anvil(root, &["orchestrator", "setup", "--executor-effort", "high"]), "partial setup");
    assert!(fs::read_to_string(&executor_path).unwrap().contains("model: haiku\neffort: high\n"));

    let local = root.join(".claude/settings.local.json");
    fs::write(&local, "{\"permissions\":{\"allow\":[]}}").unwrap();
    assert_ok(&run_anvil(root, &["orchestrator", "on", "--default-agent"]), "on");
    let on: Value = serde_json::from_str(&fs::read_to_string(&local).unwrap()).unwrap();
    assert_eq!(on["agent"], json!("anvil-orchestrator"));
    assert!(on["permissions"].is_object(), "existing local settings must survive");
    assert_ok(&run_anvil(root, &["orchestrator", "off"]), "off");
    let off: Value = serde_json::from_str(&fs::read_to_string(&local).unwrap()).unwrap();
    assert!(off.get("agent").is_none());

    fs::write(&local, "{\"agent\":\"reviewer\"}").unwrap();
    assert!(!run_anvil(root, &["orchestrator", "on", "--default-agent"]).status.success(), "must not replace the user's agent");

    fs::write(&executor_path, "---\nname: anvil-executor\n---\nmine\n").unwrap();
    assert_ok(&run_anvil(root, &["sync"]), "sync");
    assert!(fs::read_to_string(&executor_path).unwrap().ends_with("mine\n"), "user-owned agents are preserved");
}

fn role_decision(root: &Path, agent: Option<&str>, tool: &str, input: Value) -> (String, Value) {
    let mut payload = json!({
        "hook_event_name": "PreToolUse",
        "session_id": "orch-1",
        "cwd": root.to_string_lossy(),
        "tool_name": tool,
        "tool_input": input,
    });
    if let Some(kind) = agent {
        payload["agent_id"] = json!("a1");
        payload["agent_type"] = json!(kind);
    }
    let out = assert_ok(
        &run_anvil_stdin(root, &["hook", "PreToolUse", "--adapter", "claude"], &payload.to_string()),
        tool,
    );
    let parsed: Value = serde_json::from_str(&out).unwrap();
    let specific = parsed["hookSpecificOutput"].clone();
    (specific["permissionDecision"].as_str().unwrap_or("none").to_owned(), specific["updatedInput"].clone())
}

fn assert_roles(root: &Path, agent: Option<&str>, cases: &[(&str, Value, &str)]) {
    for (tool, input, expected) in cases {
        assert_eq!(role_decision(root, agent, tool, input.clone()).0, *expected, "{agent:?} {tool} {input}");
    }
}

fn file_input(root: &Path, rel: &str) -> Value {
    json!({ "file_path": root.join(rel).to_string_lossy() })
}

fn shell_input(cmd: &str) -> Value {
    json!({ "command": cmd })
}

fn executor_brief(prompt: &str) -> Value {
    json!({ "subagent_type": "anvil-executor", "prompt": prompt, "model": "opus" })
}

fn repo_in_orchestrator_mode() -> TempDir {
    let repo = setup_git_ts_repo();
    assert_ok(&run_anvil(repo.path(), &["init"]), "init");
    assert_ok(&run_anvil(repo.path(), &["orchestrator", "on"]), "on");
    repo
}

#[test]
fn orchestrator_mode_toggles_reports_status_and_reminds() {
    let repo = repo_in_orchestrator_mode();
    let root = repo.path();
    let raw = assert_ok(&run_anvil(root, &["orchestrator", "status", "--json"]), "status");
    let status: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!((&status["enabled"], &status["executor"]["model"]), (&json!(true), &json!("sonnet")));

    let prompt = json!({ "hook_event_name": "UserPromptSubmit", "session_id": "orch-1", "cwd": root.to_string_lossy(), "prompt": "hola" });
    let reminder = assert_ok(&run_anvil_stdin(root, &["hook", "user-prompt-submit", "--adapter", "claude"], &prompt.to_string()), "prompt");
    assert!(reminder.contains("You are the orchestrator"), "{reminder}");

    assert_ok(&run_anvil(root, &["orchestrator", "off"]), "off");
    assert_roles(root, None, &[("Edit", file_input(root, "src/core.ts"), "none")]);
    assert_roles(root, Some("anvil-executor"), &[("Edit", file_input(root, "src/page.tsx"), "none")]);
}

#[test]
fn orchestrator_mode_keeps_judges_away_from_code() {
    let repo = repo_in_orchestrator_mode();
    let root = repo.path();
    let core = || file_input(root, "src/core.ts");
    assert_roles(root, Some("general-purpose"), &[("Edit", core(), "deny"), ("Read", core(), "none")]);
    assert_roles(root, None, &[
        ("Edit", core(), "deny"),
        ("Write", core(), "deny"),
        ("NotebookEdit", core(), "deny"),
        ("mcp__fs__write_file", core(), "deny"),
        ("Read", core(), "none"),
        ("Bash", shell_input("git diff"), "none"),
        ("Bash", shell_input("anvil check"), "none"),
        ("Bash", shell_input("echo x > src/core.ts"), "deny"),
        ("Bash", shell_input("anvil orchestrator off"), "deny"),
        ("Bash", shell_input("python fix.py"), "deny"),
    ]);
}

#[test]
fn orchestrator_mode_limits_executors_to_the_delegated_scope() {
    let repo = repo_in_orchestrator_mode();
    let root = repo.path();
    let exec = Some("anvil-executor");
    assert_roles(root, exec, &[("Edit", file_input(root, "src/core.ts"), "deny")]);
    assert_roles(root, None, &[
        ("Agent", executor_brief("fix it"), "deny"),
        ("Agent", executor_brief("SCOPE: **"), "deny"),
    ]);
    let (decision, updated) = role_decision(root, None, "Agent", executor_brief("Fix it
SCOPE: src/core.ts, docs"));
    assert_eq!(decision, "none");
    assert!(updated.get("model").is_none() && updated["prompt"].is_string(), "configured model wins: {updated}");

    assert_roles(root, exec, &[
        ("Edit", file_input(root, "src/core.ts"), "none"),
        ("Write", file_input(root, "docs/new.md"), "none"),
        ("Edit", file_input(root, "src/page.tsx"), "deny"),
        ("Bash", shell_input("echo x > docs/a.md"), "none"),
        ("Bash", shell_input("echo x > src/page.tsx"), "deny"),
        ("Bash", shell_input("git commit -m x"), "deny"),
        ("Bash", shell_input("anvil orchestrator off"), "deny"),
        ("Agent", executor_brief("SCOPE: src"), "deny"),
        ("mcp__anvil__task_done", json!({}), "deny"),
    ]);
}

fn antigravity_hook(root: &Path, event: &str, mut payload: Value) -> String {
    payload["cwd"] = json!(root.to_string_lossy());
    assert_ok(&run_anvil_stdin(root, &["hook", event, "--adapter", "antigravity"], &payload.to_string()), event)
}

#[test]
fn orchestrator_mode_is_enforced_for_claude_code_only() {
    let repo = repo_in_orchestrator_mode();
    let root = repo.path();
    let target = root.join("src/core.ts").to_string_lossy().into_owned();

    let write = json!({
        "hookEventName": "PreToolUse",
        "toolCall": { "toolName": "write_to_file", "toolInput": { "TargetFile": target, "CodeContent": "export const x = 1;\n" } }
    });
    let decision: Value = serde_json::from_str(&antigravity_hook(root, "pre-tool-use", write)).unwrap();
    assert_eq!(decision["decision"], json!("allow"), "antigravity writes are not role-gated: {decision}");

    let destructive = json!({
        "hookEventName": "PreToolUse",
        "toolCall": { "toolName": "Bash", "toolInput": { "command": "git reset --hard HEAD" } }
    });
    let decision: Value = serde_json::from_str(&antigravity_hook(root, "pre-tool-use", destructive)).unwrap();
    assert_eq!(decision["decision"], json!("ask"), "security guard still applies: {decision}");

    let secret = json!({
        "hookEventName": "PreToolUse",
        "toolCall": { "toolName": "Read", "toolInput": { "file_path": ".env" } }
    });
    let decision: Value = serde_json::from_str(&antigravity_hook(root, "pre-tool-use", secret)).unwrap();
    assert_eq!(decision["decision"], json!("ask"), "security guard still applies: {decision}");

    let pre_invocation = antigravity_hook(root, "pre-invocation", json!({ "hookEventName": "PreInvocation", "lastUserInput": "hola" }));
    assert!(!pre_invocation.contains("orchestrator"), "{pre_invocation}");
    let prompt = antigravity_hook(root, "user-prompt-submit", json!({ "hookEventName": "UserPromptSubmit", "prompt": "hola" }));
    assert!(!prompt.contains("orchestrator"), "{prompt}");

    assert_roles(root, None, &[("Write", file_input(root, "src/core.ts"), "deny")]);
}

#[test]
fn global_flags_cannot_hide_orchestrator_off_from_either_role() {
    let repo = repo_in_orchestrator_mode();
    let root = repo.path();
    for agent in [Some("anvil-executor"), None] {
        assert_roles(root, agent, &[
            ("Bash", shell_input("anvil --root . orchestrator off"), "deny"),
            ("Bash", shell_input("anvil --root=. orchestrator off"), "deny"),
            ("Bash", shell_input("anvil --no-color orchestrator off"), "deny"),
            ("Bash", shell_input("anvil --root . check"), "none"),
        ]);
    }
    assert!(root.join(".harness/.state/orchestrator.json").is_file(), "the mode must still be on");
}
