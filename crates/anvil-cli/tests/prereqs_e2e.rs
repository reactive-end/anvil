//! `anvil prereqs`, the `init.sh`/`init.ps1` wrappers, `doctor --prereqs` and `init` end to end.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Value, json};
use tempfile::TempDir;

fn anvil(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_anvil"))
        .arg("--root")
        .arg(root)
        .args(args)
        .env("NO_COLOR", "1")
        .env("ANVIL_NO_AUTO_DAEMON", "1")
        .env("ANVIL_LANG", "en")
        .output()
        .expect("spawn anvil")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, text).expect("write");
}

fn read_config(root: &Path) -> Value {
    let text = fs::read_to_string(root.join(".harness/config.json")).expect("config");
    serde_json::from_str(&text).expect("config json")
}

/// A Node project whose `.harness/config.json` carries `prerequisites`.
fn project_with(prerequisites: Value) -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), "package.json", r#"{"name":"orders","engines":{"node":">=20"}}"#);
    let config = json!({"version": 1, "circuit_breaker_limit": 7, "prerequisites": prerequisites});
    write(dir.path(), ".harness/config.json", &config.to_string());
    dir
}

#[test]
fn detect_write_stores_the_set_and_keeps_every_other_key() {
    let dir = project_with(json!({
        "extra_tools": [{"name": "mytool", "version": ">=1", "hint": "get it"}],
        "ignore": ["gh"],
        "custom": [{"name": "db up", "command": "true"}]
    }));
    let root = dir.path();

    let out = anvil(root, &["prereqs", "detect", "--write"]);

    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout(&out).contains("node"), "detect should list node: {}", stdout(&out));
    let config = read_config(root);
    assert_eq!(config["circuit_breaker_limit"], 7);
    assert_eq!(config["prerequisites"]["extra_tools"][0]["name"], "mytool");
    assert_eq!(config["prerequisites"]["ignore"], json!(["gh"]));
    assert_eq!(config["prerequisites"]["custom"][0]["name"], "db up");
    let tools = config["prerequisites"]["detected"]["tools"].as_array().expect("tools");
    let node = tools.iter().find(|t| t["name"] == "node").expect("node detected");
    assert_eq!(node["version"], ">=20");
    assert_eq!(node["source"], "package.json engines.node");
}

#[test]
fn detect_without_write_leaves_the_config_alone() {
    let dir = project_with(json!({}));
    let before = fs::read_to_string(dir.path().join(".harness/config.json")).expect("config");

    let out = anvil(dir.path(), &["prereqs", "detect"]);

    assert!(out.status.success());
    let after = fs::read_to_string(dir.path().join(".harness/config.json")).expect("config");
    assert_eq!(before, after);
}

#[test]
fn check_json_reports_a_missing_required_tool_and_exits_1() {
    let dir = project_with(json!({
        "extra_tools": [{"name": "anvil-no-such-tool-xyz", "version": ">=1", "hint": "install it from nowhere"}],
        "ignore": ["git", "node"]
    }));

    let out = anvil(dir.path(), &["prereqs", "check", "--json"]);

    assert_eq!(out.status.code(), Some(1), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let report: Value = serde_json::from_str(&stdout(&out)).expect("json on stdout");
    assert_eq!(report["ok"], false);
    assert_eq!(report["summary"]["missing"], 1);
    let items = report["items"].as_array().expect("items");
    let missing = items.iter().find(|i| i["name"] == "anvil-no-such-tool-xyz").expect("item");
    assert_eq!(missing["status"], "missing");
    assert_eq!(missing["kind"], "tool");
    assert_eq!(missing["constraint"], ">=1");
    assert_eq!(missing["hint"], "install it from nowhere");
    assert_eq!(missing["required"], true);
    let anvil_item = items.iter().find(|i| i["name"] == "anvil").expect("anvil itself");
    assert_eq!(anvil_item["status"], "ok");
}

#[test]
fn optional_missing_tool_and_ignored_names_do_not_fail_the_check() {
    let dir = project_with(json!({
        "extra_tools": [{"name": "anvil-no-such-tool-xyz", "required": false}],
        "ignore": ["git", "node"]
    }));

    let out = anvil(dir.path(), &["prereqs", "check", "--json"]);

    assert_eq!(out.status.code(), Some(0), "stdout: {}", stdout(&out));
    let report: Value = serde_json::from_str(&stdout(&out)).expect("json");
    assert_eq!(report["ok"], true);
    assert_eq!(report["summary"]["warning"], 1);
}

#[test]
fn check_table_marks_failures_and_prints_the_hint() {
    let dir = project_with(json!({
        "extra_tools": [{"name": "anvil-no-such-tool-xyz", "hint": "install it from nowhere"}],
        "ignore": ["git", "node"]
    }));

    let out = anvil(dir.path(), &["prereqs", "check"]);

    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1));
    assert!(text.contains("✗ anvil-no-such-tool-xyz"), "table: {text}");
    assert!(text.contains("hint: install it from nowhere"), "table: {text}");
    assert!(text.contains("✓ anvil"), "table: {text}");
}

#[test]
fn check_without_stored_detection_detects_on_the_fly_and_filters_by_subrepo() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write(root, "api/package.json", r#"{"engines":{"node":">=1"}}"#);
    write(root, "web/package.json", r#"{"engines":{"node":">=99999"}}"#);
    write(root, ".harness/config.json", &json!({"prerequisites": {"ignore": ["git"]}}).to_string());

    let all = anvil(root, &["prereqs", "check", "--json"]);
    let report: Value = serde_json::from_str(&stdout(&all)).expect("json");
    let groups: Vec<&str> = report["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|i| i["group"].as_str())
        .collect();
    assert!(groups.contains(&"api") && groups.contains(&"web"), "groups: {groups:?}");

    let only_api = anvil(root, &["prereqs", "check", "--json", "--subrepo", "api"]);
    let report: Value = serde_json::from_str(&stdout(&only_api)).expect("json");
    assert!(report["items"].as_array().expect("items").iter().all(|i| i["group"] != "web"));

    let unknown = anvil(root, &["prereqs", "check", "--subrepo", "nope"]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("unknown subrepo"));
}

#[test]
fn script_writes_both_wrappers_and_allows_them_at_the_root() {
    let dir = project_with(json!({}));
    let root = dir.path();
    let stored = json!({"version": 1, "root_allowlist": ["README.md", "package.json"]});
    write(root, ".harness/config.json", &stored.to_string());

    let out = anvil(root, &["prereqs", "script"]);

    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let sh = fs::read_to_string(root.join("init.sh")).expect("init.sh");
    assert!(sh.starts_with("#!/bin/sh"));
    assert!(sh.contains("command -v anvil"));
    assert!(sh.contains("scripts/install.sh"), "install instructions from the README");
    assert!(sh.contains("anvil prereqs check \"$@\""));
    assert!(sh.contains("exit $?"), "the exit code is propagated");
    let ps1 = fs::read_to_string(root.join("init.ps1")).expect("init.ps1");
    assert!(ps1.contains("Get-Command anvil"));
    assert!(ps1.contains("scripts\\install.ps1"));
    assert!(ps1.contains("anvil prereqs check @args"));
    assert!(ps1.contains("exit $LASTEXITCODE"));
    let allow = read_config(root)["root_allowlist"].clone();
    for name in ["init.sh", "init.ps1", "init.cmd"] {
        assert!(
            allow.as_array().expect("allowlist").iter().any(|a| a == name),
            "{name} in {allow}"
        );
    }

    let sub = root.join("tools");
    let out = anvil(root, &["prereqs", "script", "--out", &sub.to_string_lossy()]);
    assert!(out.status.success());
    assert!(sub.join("init.sh").is_file() && sub.join("init.ps1").is_file());
}

#[test]
fn init_sh_runs_the_check_and_propagates_its_exit_code() {
    let dir = project_with(json!({
        "extra_tools": [{"name": "anvil-no-such-tool-xyz"}],
        "ignore": ["git", "node"]
    }));
    let root = dir.path();
    assert!(anvil(root, &["prereqs", "script"]).status.success());
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_anvil")).parent().expect("bin dir");
    let path = std::env::join_paths(
        std::iter::once(bin_dir.to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
    )
    .expect("path");

    let Ok(out) = Command::new("sh").arg("init.sh").current_dir(root).env("PATH", path).output()
    else {
        return;
    };

    assert_eq!(out.status.code(), Some(1), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout(&out).contains("anvil-no-such-tool-xyz"), "{}", stdout(&out));
}

#[test]
fn init_stores_the_detection_and_hints_at_the_check() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write(root, "package.json", r#"{"name":"orders","engines":{"node":">=20"}}"#);
    write(root, ".env.example", "A=\n");

    let out = anvil(root, &["init", "--adapter", "claude", "--sonar", "no"]);

    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = stdout(&out);
    assert!(text.contains("anvil prereqs check"), "init output: {text}");
    let config = read_config(root);
    let detected = &config["prerequisites"]["detected"];
    assert!(detected["tools"].as_array().expect("tools").iter().any(|t| t["name"] == "node"));
    assert_eq!(detected["env_files"][0]["example"], ".env.example");
}

#[test]
fn doctor_shows_a_summary_and_prereqs_flag_fails_on_required_items() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write(root, "package.json", r#"{"name":"orders"}"#);
    assert!(anvil(root, &["init", "--adapter", "claude", "--sonar", "no"]).status.success());

    let plain = anvil(root, &["doctor"]);
    assert!(stdout(&plain).contains("prereqs:"), "doctor output: {}", stdout(&plain));
    let baseline = plain.status.code();

    let mut config = read_config(root);
    config["prerequisites"]["extra_tools"] = json!([{"name": "anvil-no-such-tool-xyz"}]);
    write(root, ".harness/config.json", &config.to_string());

    let still_same = anvil(root, &["doctor"]);
    assert_eq!(still_same.status.code(), baseline, "the summary alone never changes doctor's exit");
    let strict = anvil(root, &["doctor", "--prereqs"]);
    assert_eq!(strict.status.code(), Some(1));
    assert!(stdout(&strict).contains("anvil-no-such-tool-xyz"), "{}", stdout(&strict));
    assert!(String::from_utf8_lossy(&strict.stderr).contains("critical issue"));
}
