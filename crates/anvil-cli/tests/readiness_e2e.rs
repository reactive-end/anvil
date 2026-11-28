//! Session start tells the agent which tools the repository needs and are missing: MCP Toolbox for
//! a project that uses a database, a local SonarQube where Sonar is enabled.

use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

fn anvil(root: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_anvil"))
        .arg("--root")
        .arg(root)
        .args(args)
        .env("NO_COLOR", "1")
        .env("ANVIL_LANG", "en")
        .env("ANVIL_NO_AUTO_DAEMON", "1")
        .env("ANVIL_HOME", root.join("global-anvil"))
        .env("SONAR_SCANNER_BIN", root.join("no-scanner.exe"))
        .env("PATH", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn anvil");
    if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(data.as_bytes()).expect("stdin");
    }
    child.wait_with_output().expect("wait")
}

fn session_context(root: &Path) -> String {
    let payload = json!({ "hook_event_name": "SessionStart", "session_id": "r", "cwd": root });
    let out =
        anvil(root, &["hook", "SessionStart", "--adapter", "claude"], Some(&payload.to_string()));
    let parsed: Value = serde_json::from_slice(&out.stdout).expect("hook json");
    parsed["hookSpecificOutput"]["additionalContext"].as_str().unwrap_or_default().to_owned()
}

fn repo(package_json: &str) -> TempDir {
    let dir = TempDir::new().expect("tmp");
    fs::create_dir_all(dir.path().join(".harness")).expect("harness");
    fs::write(dir.path().join("package.json"), package_json).expect("package.json");
    dir
}

#[test]
fn a_project_with_a_database_driver_is_told_to_set_up_toolbox() {
    let dir = repo(r#"{"name":"api","dependencies":{"pg":"8"}}"#);
    let context = session_context(dir.path());
    assert!(context.contains("[anvil-readiness]"), "{context}");
    assert!(context.contains("toolbox"), "{context}");
    assert!(context.contains("anvil db init --kind postgres"), "{context}");
    assert!(context.contains("anvil db mcp on"), "{context}");
}

#[test]
fn sonar_enabled_needs_a_scanner_and_a_server() {
    let dir = repo(r#"{"name":"web","devDependencies":{"vite":"8"}}"#);
    let config =
        json!({ "version": 1, "sonar": { "enabled": true, "host_url": "http://127.0.0.1:9" } });
    fs::write(dir.path().join(".harness/config.json"), config.to_string()).expect("config");
    let context = session_context(dir.path());
    assert!(context.contains("sonar-scanner"), "{context}");
    assert!(context.contains("no SonarQube answers at http://127.0.0.1:9"), "{context}");
    assert!(!context.contains("toolbox"), "no database, no Toolbox: {context}");
}

#[test]
fn a_ready_project_gets_no_warning() {
    let dir = repo(r#"{"name":"web","devDependencies":{"vite":"8"}}"#);
    let context = session_context(dir.path());
    assert!(!context.contains("toolbox") && !context.contains("sonar"), "{context}");
}
