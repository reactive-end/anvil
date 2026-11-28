//! The `Stop` gate for backend changes: a turn that edits server code of a project that uses a
//! database only ends once the API scenarios passed after the edit. Each missing piece (the
//! Toolbox tools file, a scenario, a fresh passing run) blocks with the command that fixes it.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;

use serde_json::json;
use tempfile::TempDir;

fn anvil(root: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_anvil"))
        .arg("--root")
        .arg(root)
        .args(args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        .env("ANVIL_LANG", "en")
        .env("ANVIL_NO_AUTO_DAEMON", "1")
        .env("ANVIL_HOME", root.join("anvil-home"))
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

fn ok(root: &Path, args: &[&str]) -> String {
    let out = anvil(root, args, None);
    let text =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "anvil {args:?} failed: {text}");
    text
}

fn stop(root: &Path) -> String {
    let payload = json!({ "hook_event_name": "Stop", "session_id": "gate", "cwd": root });
    let out = anvil(root, &["hook", "Stop", "--adapter", "claude"], Some(&payload.to_string()));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, text).expect("write");
}

/// Answers every request with `200 {"ok":true}`.
fn serve_api() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://127.0.0.1:{}", listener.local_addr().expect("addr").port());
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            let body = r#"{"ok":true}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    url
}

const SERVICE: &str = "export function total(cents: number): number {\n  return cents;\n}\n";
const SCENARIO: &str = r#"{ "name": "health", "steps": [
  { "http": { "method": "GET", "path": "/health" }, "expect": { "status": 200 } } ] }"#;

#[test]
fn backend_edits_need_the_database_setup_a_scenario_and_a_fresh_passing_run() {
    let dir = TempDir::new().expect("tmp");
    let root = dir.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "t@example.com"]);
    git(root, &["config", "user.name", "Tester"]);
    write(root, ".gitignore", "anvil-home/\n.harness/.state/\n");
    write(root, "package.json", r#"{"name":"api","dependencies":{"better-sqlite3":"11"}}"#);
    write(root, ".env", "PORT=3000\nDB_PATH=data/app.sqlite\n");
    write(root, "src/orders.ts", SERVICE);
    let config = json!({ "qa": { "api": { "url": serve_api(), "ready_path": "/health" } } });
    write(root, ".harness/config.json", &config.to_string());
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "initial"]);
    ok(root, &["tasks", "skip", "gate", "test"]);

    write(root, "src/orders.ts", &SERVICE.replace("cents;", "cents + 1;"));
    let no_database = stop(root);
    assert!(no_database.contains("Backend change not verified"), "{no_database}");
    assert!(
        no_database.contains("src/orders.ts") && no_database.contains("anvil db init"),
        "{no_database}"
    );

    let init = ok(root, &["db", "init"]);
    assert!(init.contains("detected sqlite from dependencies"), "{init}");
    assert!(init.contains("data/app.sqlite"), "{init}");
    let no_scenario = stop(root);
    assert!(no_scenario.contains("Add a scenario"), "{no_scenario}");

    write(root, ".harness/qa/api/health.json", SCENARIO);
    let not_run = stop(root);
    assert!(not_run.contains("anvil qa api --start"), "{not_run}");

    assert!(ok(root, &["qa", "api"]).contains("1 of 1 scenario(s) passed"));
    let verified = stop(root);
    assert!(!verified.contains("Backend change"), "{verified}");

    write(root, "src/orders.ts", &SERVICE.replace("cents;", "cents + 2;"));
    let stale = stop(root);
    assert!(stale.contains("Backend change not verified"), "{stale}");

    write(root, "src/orders.spec.ts", "it('adds', () => {});\n");
    ok(root, &["qa", "api"]);
    assert!(!stop(root).contains("Backend change"), "a test file alone never needs the database");
}
