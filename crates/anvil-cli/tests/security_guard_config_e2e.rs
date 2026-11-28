//! E2E tests for the configurable security guard: hook decisions, `anvil guard` commands, and the
//! Claude Code permission rules derived from the configuration.

use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

const TERMINATE: &str = "aws ec2 terminate-instances --instance-ids i-123";

fn run_anvil(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_anvil"))
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env("ANVIL_NO_AUTO_DAEMON", "1")
        .output()
        .expect("run anvil")
}

fn run_ok(cwd: &Path, args: &[&str]) -> String {
    let out = run_anvil(cwd, args);
    assert!(
        out.status.success(),
        "anvil {args:?} failed ({:?}):\nstdout={}\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn hook(cwd: &Path, payload: &Value) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_anvil"))
        .args(["hook", "pre-tool-use", "--adapter", "claude"])
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env("ANVIL_NO_AUTO_DAEMON", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn anvil hook");
    child.stdin.as_mut().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "hook failed: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("hook JSON")
}

fn payload(root: &Path, tool: &str, tool_input: Value) -> Value {
    json!({
        "hook_event_name": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "tool_name": tool,
        "tool_input": tool_input,
    })
}

fn bash(root: &Path, command: &str) -> Value {
    payload(root, "Bash", json!({ "command": command }))
}

fn decision(out: &Value) -> Option<&str> {
    out["hookSpecificOutput"]["permissionDecision"].as_str()
}

fn init_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    Command::new("git").args(["init"]).current_dir(dir.path()).output().unwrap();
    fs::write(dir.path().join("package.json"), r#"{"name":"guard-demo","version":"1.0.0"}"#)
        .unwrap();
    run_ok(dir.path(), &["init"]);
    dir
}

fn settings_rules(root: &Path, list: &str) -> Vec<String> {
    let raw = fs::read_to_string(root.join(".claude/settings.json")).expect("settings.json");
    let settings: Value = serde_json::from_str(&raw).unwrap();
    settings["permissions"][list]
        .as_array()
        .map(|rules| rules.iter().filter_map(|r| r.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

#[test]
fn cloud_commands_are_denied_by_default_and_follow_anvil_guard_set() {
    let repo = init_repo();
    let root = repo.path();

    let denied = hook(root, &bash(root, TERMINATE));
    assert_eq!(decision(&denied), Some("deny"));
    let reason = denied["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
    assert!(reason.contains("anvil guard set cloud ask"), "{reason}");
    assert!(
        settings_rules(root, "deny").iter().any(|r| r == "Bash(aws ec2 terminate-instances:*)")
    );

    let mcp = payload(
        root,
        "mcp__aws__call_aws",
        json!({ "cli_command": "aws s3 rb s3://bucket --force" }),
    );
    assert_eq!(decision(&hook(root, &mcp)), Some("deny"));
    let sql = payload(root, "mcp__db__execute_sql", json!({ "sql": "DELETE FROM users" }));
    assert_eq!(decision(&hook(root, &sql)), Some("ask"));
    let filtered =
        payload(root, "mcp__db__execute_sql", json!({ "sql": "DELETE FROM users WHERE id = 1" }));
    assert_eq!(decision(&hook(root, &filtered)), None);
    assert_eq!(decision(&hook(root, &bash(root, "aws s3 ls"))), None);

    run_ok(root, &["guard", "set", "cloud", "ask"]);
    assert_eq!(decision(&hook(root, &bash(root, TERMINATE))), Some("ask"));
    assert!(settings_rules(root, "ask").iter().any(|r| r == "Bash(aws ec2 terminate-instances:*)"));
    assert!(!settings_rules(root, "deny").iter().any(|r| r.starts_with("Bash(aws ")));

    run_ok(root, &["guard", "set", "cloud", "allow"]);
    assert_eq!(decision(&hook(root, &bash(root, TERMINATE))), None);
    assert!(!settings_rules(root, "ask").iter().any(|r| r.starts_with("Bash(aws ")));

    run_ok(root, &["guard", "reset"]);
    assert_eq!(decision(&hook(root, &bash(root, TERMINATE))), Some("deny"));
}

#[test]
fn the_agent_cannot_relax_its_own_guard() {
    let repo = init_repo();
    let root = repo.path();
    for cmd in [
        "anvil guard set cloud allow",
        "anvil guard add allow '.*'",
        "anvil guard remove x",
        "anvil guard reset",
        "sudo anvil --root . guard set infra allow",
    ] {
        assert_eq!(decision(&hook(root, &bash(root, cmd))), Some("ask"), "{cmd}");
    }
    run_ok(root, &["guard", "set", "cloud", "allow"]);
    let relax_again = bash(root, "anvil guard set infra allow");
    assert_eq!(decision(&hook(root, &relax_again)), Some("ask"), "stays ask even when allowed");
    for cmd in ["anvil guard list", "anvil guard test \"terraform destroy\""] {
        assert_eq!(decision(&hook(root, &bash(root, cmd))), None, "{cmd}");
    }

    let edit = payload(
        root,
        "Edit",
        json!({
            "file_path": ".harness/config.json",
            "old_string": "\"cloud\": \"deny\"",
            "new_string": "\"cloud\": \"allow\"",
        }),
    );
    assert_eq!(decision(&hook(root, &edit)), Some("ask"));
    let write = payload(
        root,
        "Write",
        json!({ "file_path": ".harness/config.json", "content": "{\"security_guard\": {}}" }),
    );
    assert_eq!(decision(&hook(root, &write)), Some("ask"));
    let other = payload(root, "Write", json!({ "file_path": "notes.md", "content": "hello" }));
    assert_eq!(decision(&hook(root, &other)), None);
}

#[test]
fn guard_test_prints_decision_category_and_reason_without_running_anything() {
    let repo = init_repo();
    let root = repo.path();

    let denied = run_ok(root, &["guard", "test", TERMINATE]);
    assert!(denied.contains("decision: deny"), "{denied}");
    assert!(denied.contains("category: cloud"), "{denied}");
    assert!(denied.contains("anvil guard set cloud ask"), "{denied}");

    let asked = run_ok(root, &["guard", "test", "git push --force-with-lease origin main"]);
    assert!(asked.contains("decision: ask"), "{asked}");
    assert!(asked.contains("category: git"), "{asked}");

    let routine = run_ok(root, &["guard", "test", "kubectl get pods"]);
    assert!(routine.contains("decision: allow"), "{routine}");
    assert!(routine.contains("category: -"), "{routine}");

    let mcp = run_ok(
        root,
        &["guard", "test", "aws s3 rb s3://bucket --force", "--tool", "mcp__aws__call_aws"],
    );
    assert!(mcp.contains("decision: deny"), "{mcp}");

    let sql = run_ok(root, &["guard", "test", "TRUNCATE users", "--tool", "mcp__db__execute_sql"]);
    assert!(sql.contains("decision: ask") && sql.contains("category: database"), "{sql}");

    let guard = run_ok(root, &["guard", "test", "anvil guard set cloud allow"]);
    assert!(guard.contains("decision: ask"), "{guard}");
}

#[test]
fn guard_patterns_precede_categories_and_invalid_regexes_are_reported_not_fatal() {
    let repo = init_repo();
    let root = repo.path();
    let config_path = root.join(".harness/config.json");
    let mut config: Value =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    config["custom_key"] = json!({ "keep": true });
    fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    run_ok(root, &["guard", "add", "deny", "^kubectl apply"]);
    run_ok(root, &["guard", "add", "allow", "terraform destroy -target=module\\.sandbox$"]);
    let test = |cmd: &str| run_ok(root, &["guard", "test", cmd]);
    assert!(test("kubectl apply -f x.yaml").contains("decision: deny"));
    assert!(test("terraform destroy -target=module.sandbox").contains("decision: allow"));
    assert!(test("terraform destroy -target=module.prod").contains("decision: deny"));
    assert_eq!(decision(&hook(root, &bash(root, "kubectl apply -f x.yaml"))), Some("deny"));

    let saved: Value = serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(saved["custom_key"], json!({ "keep": true }));
    assert_eq!(saved["security_guard"]["deny_patterns"], json!(["^kubectl apply"]));
    assert_eq!(saved["security_guard"]["categories"]["cloud"], json!("deny"));

    run_ok(root, &["guard", "remove", "^kubectl apply"]);
    assert!(test("kubectl apply -f x.yaml").contains("decision: allow"));
}

#[test]
fn invalid_regexes_are_rejected_when_added_and_reported_but_never_fatal_when_hand_written() {
    let repo = init_repo();
    let root = repo.path();
    let config_path = root.join(".harness/config.json");
    run_ok(root, &["guard", "add", "deny", "^kubectl apply"]);

    let rejected = run_anvil(root, &["guard", "add", "deny", "([unclosed"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("invalid regex"));
    assert!(!run_anvil(root, &["guard", "remove", "never-added"]).status.success());

    let mut config: Value =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    config["security_guard"]["ask_patterns"] = json!(["([broken"]);
    fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    let listing = run_ok(root, &["guard", "list"]);
    assert!(listing.contains("cloud") && listing.contains("deny"), "{listing}");
    assert!(listing.contains("^kubectl apply"), "{listing}");
    assert!(listing.contains("([broken") && listing.contains("INVALID"), "{listing}");
    assert_eq!(decision(&hook(root, &bash(root, "aws s3 ls"))), None, "invalid regex is ignored");
    assert_eq!(decision(&hook(root, &bash(root, "kubectl apply -f x"))), Some("deny"));
}
