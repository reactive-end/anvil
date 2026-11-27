//! `anvil qa api` and `anvil db` end to end.
//!
//! The first tests need nothing but this repository: a local HTTP server stands in for the API and
//! no Toolbox binary is involved. The last one drives the real hexagonal sandbox (NestJS API plus
//! the Toolbox binary) and is ignored by default:
//!
//! ```text
//! cargo test -p anvil-cli --test api_qa_e2e -- --ignored --nocapture
//! ```
//!
//! It starts `orders-api` on port 3100 with a throwaway SQLite file, so the port must be free.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;

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

fn text(out: &Output) -> String {
    format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, content).expect("write");
}

/// Reads one request, including its body, so the reply is not cut by a connection reset.
fn read_request(stream: &mut impl Read) -> String {
    let mut data = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream.read(&mut chunk).unwrap_or(0);
        data.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&data).into_owned();
        let Some(head_end) = text.find("\r\n\r\n") else {
            if read == 0 {
                return text;
            }
            continue;
        };
        let length = text[..head_end]
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase().strip_prefix("content-length:")?.trim().parse().ok()
            })
            .unwrap_or(0usize);
        if read == 0 || data.len() >= head_end + 4 + length {
            return text;
        }
    }
}

/// A server that answers `GET /health` and `POST /customers` like a tiny API.
fn serve_api() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://127.0.0.1:{}", listener.local_addr().expect("addr").port());
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let request = read_request(&mut stream);
            let (status, body) = if request.starts_with("POST /customers") {
                ("201 Created", r#"{"id":"c-1","email":"qa@example.com"}"#)
            } else {
                ("200 OK", r#"{"ok":true}"#)
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    url
}

fn project(url: &str) -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    write(
        dir.path(),
        ".harness/config.json",
        &json!({ "qa": { "api": { "url": url, "ready_path": "/health" } } }).to_string(),
    );
    dir
}

const SCENARIO: &str = r#"{
  "name": "create customer",
  "steps": [
    { "http": { "method": "POST", "path": "/customers", "json": { "email": "qa@example.com" } },
      "expect": { "status": 201, "json": { "/email": "qa@example.com" } },
      "capture": { "id": "/id" } },
    { "http": { "method": "GET", "path": "/health" }, "expect": { "status": 200 } }
  ]
}"#;

#[test]
fn scenarios_run_pass_and_fail_with_the_expected_and_actual_values() {
    let dir = project(&serve_api());
    let root = dir.path();
    write(root, ".harness/qa/api/create-customer.json", SCENARIO);

    let listed = anvil(root, &["qa", "api", "--list"]);
    assert!(listed.status.success(), "{}", text(&listed));
    assert!(text(&listed).contains("create-customer"), "{}", text(&listed));

    let passed = anvil(root, &["qa", "api"]);
    assert_eq!(passed.status.code(), Some(0), "{}", text(&passed));
    assert!(text(&passed).contains("1 of 1 scenario(s) passed"), "{}", text(&passed));
    let reports: Vec<PathBuf> = fs::read_dir(root.join(".harness/.state/qa/api"))
        .expect("runs dir")
        .map(|entry| entry.expect("entry").path().join("report.json"))
        .collect();
    let report: Value =
        serde_json::from_str(&fs::read_to_string(&reports[0]).expect("report")).expect("json");
    assert_eq!(report["passed"], json!(true));

    write(root, ".harness/qa/api/create-customer.json", &SCENARIO.replace("201", "200"));
    let failed = anvil(root, &["qa", "api", "create-customer", "--json"]);
    assert_eq!(failed.status.code(), Some(1), "{}", text(&failed));
    let run: Value = serde_json::from_slice(&failed.stdout).expect("json report");
    let step = &run["scenarios"][0]["steps"][0];
    assert_eq!(step["status"], json!("failed"));
    assert_eq!(step["reason"], json!("expected status 200 but got 201"));
    assert_eq!(step["actual"]["id"], json!("c-1"));
}

#[test]
fn an_unreachable_api_and_unknown_names_fail_with_a_clear_message() {
    let dir = project("http://127.0.0.1:1");
    write(dir.path(), ".harness/qa/api/create-customer.json", SCENARIO);
    let down = anvil(dir.path(), &["qa", "api"]);
    assert_eq!(down.status.code(), Some(1));
    assert!(
        text(&down).contains("the API does not answer at http://127.0.0.1:1/health"),
        "{}",
        text(&down)
    );
    let unknown = anvil(dir.path(), &["qa", "api", "nope"]);
    assert_eq!(unknown.status.code(), Some(1));
    assert!(text(&unknown).contains("no scenario named `nope`"), "{}", text(&unknown));
}

#[test]
fn db_init_writes_the_tools_file_and_refuses_to_overwrite_it() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    let no_path = anvil(root, &["db", "init", "--kind", "sqlite"]);
    assert_eq!(no_path.status.code(), Some(1));
    assert!(text(&no_path).contains("--database"), "{}", text(&no_path));

    write(root, "api/.env", "PORT=3000\nSECRET_KEY=do-not-print\nDB_PATH=data/app.sqlite\n");
    let found = anvil(root, &["db", "init", "--kind", "sqlite"]);
    assert!(found.status.success(), "{}", text(&found));
    assert!(text(&found).contains("api/data/app.sqlite"), "{}", text(&found));
    assert!(!text(&found).contains("do-not-print"), "{}", text(&found));
    let yaml = fs::read_to_string(root.join(".harness/db/tools.yaml")).expect("tools file");
    assert!(yaml.contains("database: api/data/app.sqlite"), "{yaml}");

    let again = anvil(root, &["db", "init", "--kind", "sqlite", "--database", "x.db"]);
    assert_eq!(again.status.code(), Some(1));
    assert!(text(&again).contains("--force"), "{}", text(&again));
    let forced = anvil(root, &["db", "init", "--kind", "postgres", "--force"]);
    assert!(forced.status.success(), "{}", text(&forced));
    let yaml = fs::read_to_string(root.join(".harness/db/tools.yaml")).expect("tools file");
    assert!(yaml.contains("password: ${DB_PASSWORD}"), "{yaml}");
}

#[test]
fn db_check_flags_literal_passwords_without_printing_them() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write(
        root,
        ".harness/config.json",
        &json!({ "db": { "toolbox": "./no-such-toolbox" } }).to_string(),
    );
    write(
        root,
        ".harness/db/tools.yaml",
        "kind: source\nname: app-db\ntype: postgres\npassword: hunter2\n",
    );
    let check = anvil(root, &["db", "check"]);
    let out = text(&check);
    assert_eq!(check.status.code(), Some(1), "{out}");
    assert!(out.contains("`password` holds a literal value"), "{out}");
    assert!(out.contains("toolbox"), "{out}");
    assert!(!out.contains("hunter2"), "{out}");
}

#[test]
fn db_query_only_reads() {
    let dir = TempDir::new().expect("tempdir");
    for sql in
        ["DELETE FROM orders", "SELECT 1; DROP TABLE orders", "UPDATE orders SET status = 'x'"]
    {
        let out = anvil(dir.path(), &["db", "query", sql]);
        assert_eq!(out.status.code(), Some(1), "{sql}");
        assert!(text(&out).contains("only"), "{sql}: {}", text(&out));
    }
}

#[test]
fn db_mcp_registers_and_removes_the_toolbox_server() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write(root, ".mcp.json", r#"{"mcpServers":{"postgres":{"command":"pg-mcp"}}}"#);
    let on = anvil(root, &["db", "mcp", "on"]);
    assert!(on.status.success(), "{}", text(&on));
    let servers: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".mcp.json")).expect("mcp"))
            .expect("json");
    assert_eq!(servers["mcpServers"]["anvil-db"]["args"][0], json!("--stdio"));
    assert!(servers["mcpServers"]["postgres"].is_object());
    let off = anvil(root, &["db", "mcp", "off"]);
    assert!(off.status.success(), "{}", text(&off));
    let servers: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".mcp.json")).expect("mcp"))
            .expect("json");
    assert!(servers["mcpServers"].get("anvil-db").is_none());
    assert!(servers["mcpServers"]["postgres"].is_object());
}

fn git_init(root: &Path) {
    let out = Command::new("git").arg("init").arg("-q").current_dir(root).output().expect("git");
    assert!(out.status.success(), "git init failed");
}

#[test]
fn a_plain_check_raises_to_tier2_and_runs_the_scenarios_for_changes_under_the_api_paths() {
    let url = serve_api();
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    git_init(root);
    let config =
        json!({ "qa": { "api": { "url": url, "ready_path": "/health", "paths": ["api"] } } });
    write(root, ".harness/config.json", &config.to_string());
    write(root, ".harness/qa/api/create-customer.json", SCENARIO);
    write(root, "api/notes.md", "# API notes\n");
    write(root, "web/notes.md", "# Web notes\n");

    let raised = anvil(root, &["check", "--scope", "api"]);
    let out = text(&raised);
    assert_eq!(raised.status.code(), Some(0), "{out}");
    assert!(out.contains("tier raised to Tier2: qa.api.paths touched"), "{out}");
    assert!(out.contains("[PASS] create-customer"), "{out}");

    let pinned = text(&anvil(root, &["check", "--scope", "api", "--tier", "1"]));
    assert!(!pinned.contains("tier raised"), "{pinned}");
    assert!(!pinned.contains("create-customer"), "{pinned}");

    let elsewhere = text(&anvil(root, &["check", "--scope", "web"]));
    assert!(!elsewhere.contains("tier raised"), "{elsewhere}");
    assert!(!elsewhere.contains("create-customer"), "{elsewhere}");

    write(root, ".harness/qa/api/create-customer.json", &SCENARIO.replace("201", "200"));
    let failing = anvil(root, &["check", "--scope", "api"]);
    assert_eq!(failing.status.code(), Some(1), "{}", text(&failing));
    assert!(text(&failing).contains("expected status 200 but got 201"), "{}", text(&failing));
}

fn sandbox() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/hexagonal-sandbox")
}

#[test]
// anvil-allow(no-test-tampering): needs Node.js, the sandbox dependencies, Toolbox and free port 3100; run with --ignored [T-053]
#[ignore = "needs Node.js, the sandbox dependencies and build, .tools/toolbox.exe, and free port 3100"]
fn scenarios_check_the_database_of_the_hexagonal_sandbox() {
    let api_dir = sandbox().join("orders-api").canonicalize().expect("orders-api");
    let toolbox = sandbox().join(".tools/toolbox.exe").canonicalize().expect("toolbox binary");
    let db_dir = TempDir::new().expect("tempdir");
    let db = db_dir.path().join("orders.sqlite");
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    let start = if cfg!(windows) {
        format!("set \"DB_PATH={}\"&& node dist/main.js", db.display())
    } else {
        format!("DB_PATH='{}' node dist/main.js", db.display())
    };
    let api = json!({
        "url": "http://localhost:3100",
        "ready_path": "/health",
        "start_command": start,
        "start_cwd": api_dir,
    });
    write(root, ".harness/config.json", &json!({ "qa": { "api": api } }).to_string());
    for entry in fs::read_dir(sandbox().join(".harness/qa/api")).expect("sandbox scenarios") {
        let path = entry.expect("entry").path();
        let target = root.join(".harness/qa/api").join(path.file_name().expect("name"));
        fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
        fs::copy(&path, target).expect("copy scenario");
    }
    let db_arg = db.to_string_lossy().replace('\\', "/");
    let init = anvil(
        root,
        &[
            "db",
            "init",
            "--kind",
            "sqlite",
            "--database",
            &db_arg,
            "--toolbox",
            &toolbox.to_string_lossy(),
        ],
    );
    assert!(init.status.success(), "{}", text(&init));

    let run = anvil(root, &["qa", "api", "--start"]);
    println!("{}", text(&run));
    assert_eq!(run.status.code(), Some(0), "{}", text(&run));
    assert!(text(&run).contains("3 of 3 scenario(s) passed"), "{}", text(&run));
    let query = anvil(root, &["db", "query", "SELECT status FROM orders WHERE status = 'paid'"]);
    assert!(text(&query).contains("\"paid\""), "{}", text(&query));

    let broken = fs::read_to_string(root.join(".harness/qa/api/create-and-pay-order.json"))
        .expect("scenario")
        .replace("\"row\": { \"status\": \"paid\" }", "\"row\": { \"status\": \"shipped\" }");
    write(root, ".harness/qa/api/create-and-pay-order.json", &broken);
    let failed = anvil(root, &["qa", "api", "create-and-pay-order", "--start"]);
    println!("{}", text(&failed));
    assert_eq!(failed.status.code(), Some(1));
    assert!(text(&failed).contains("expected \"shipped\" but got \"paid\""), "{}", text(&failed));
}
