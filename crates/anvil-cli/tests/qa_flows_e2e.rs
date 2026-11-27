//! `anvil qa` flows, visual baselines, and `--start` against the real hexagonal sandbox.
//!
//! Needs Node.js, Playwright with Chromium, and the sandbox dependencies installed, so it is ignored
//! by default. Run it with:
//!
//! ```text
//! cargo test -p anvil-cli --test qa_flows_e2e -- --ignored --nocapture
//! ```
//!
//! It starts `orders-api` (port 3100, a throwaway SQLite file) and `orders-web` (port 5180, through
//! `anvil qa run --start`), so both ports must be free.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use anvil_qa::flows::server::AppProcess;
use serde_json::{Value, json};
use tempfile::TempDir;

fn sandbox() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/hexagonal-sandbox")
}

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

fn run_json(root: &Path, args: &[&str]) -> (i32, Value, String) {
    let out = anvil(root, args);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (out.status.code().unwrap_or(-1), json, text)
}

fn flow<'a>(run: &'a Value, id: &str) -> &'a Value {
    run["flows"].as_array().and_then(|f| f.iter().find(|f| f["id"] == id)).expect("flow in report")
}

fn matched(flow: &Value) -> Vec<(String, String, i64)> {
    flow["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .flat_map(|s| s["matched_requests"].as_array().cloned().unwrap_or_default())
        .map(|r| {
            (
                r["method"].as_str().unwrap_or_default().to_owned(),
                r["url"].as_str().unwrap_or_default().to_owned(),
                r["status"].as_i64().unwrap_or(-1),
            )
        })
        .collect()
}

fn start_api(db: &Path) -> AppProcess {
    let api = sandbox().join("orders-api");
    let seed = Command::new(if cfg!(windows) { "npm.cmd" } else { "npm" })
        .args(["run", "seed"])
        .current_dir(&api)
        .env("DB_PATH", db)
        .output()
        .expect("npm run seed");
    assert!(seed.status.success(), "seed failed: {}", String::from_utf8_lossy(&seed.stderr));
    let command = if cfg!(windows) {
        format!("set \"DB_PATH={}\"&& node dist/main.js", db.display())
    } else {
        format!("DB_PATH='{}' node dist/main.js", db.display())
    };
    let log = db.with_extension("api.log");
    let mut app = AppProcess::start(&command, &api, &log).expect("start orders-api");
    app.wait_ready("http://localhost:3100/health", Duration::from_secs(60)).expect("api ready");
    app
}

fn project(web_dir: &Path) -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let config = json!({"qa": {"web": {
        "url": "http://localhost:5180",
        "start_command": "npm run dev",
        "start_cwd": web_dir,
        "ready_timeout_secs": 90,
    }}});
    fs::create_dir_all(dir.path().join(".harness/qa/flows")).expect("mkdir");
    fs::write(dir.path().join(".harness/config.json"), config.to_string()).expect("config");
    for entry in fs::read_dir(sandbox().join(".harness/qa/flows")).expect("sandbox flows") {
        let path = entry.expect("entry").path();
        let target = dir.path().join(".harness/qa/flows").join(path.file_name().expect("name"));
        fs::copy(&path, target).expect("copy flow");
    }
    dir
}

#[test]
// anvil-allow(no-test-tampering): needs node, Playwright and the sandbox servers; run with --ignored [T-052]
#[ignore = "needs Node.js, Playwright + Chromium, the sandbox dependencies, and free ports 3100/5180"]
fn flows_baselines_and_start_against_the_hexagonal_sandbox() {
    let web_dir = sandbox().join("orders-web").canonicalize().expect("orders-web");
    let db_dir = TempDir::new().expect("tempdir");
    let _api = start_api(&db_dir.path().join("orders.sqlite"));
    let work = project(&web_dir);
    let root = work.path();

    let (code, run, text) = run_json(root, &["qa", "run", "--all-flows", "--start", "--json"]);
    assert_eq!(code, 0, "first run failed: {text}");
    let create = matched(flow(&run, "create-order"));
    assert!(create.iter().any(|(m, u, s)| m == "POST" && u.ends_with("/orders") && *s == 201));
    let invalid = matched(flow(&run, "invalid-quantity"));
    assert!(invalid.iter().any(|(m, u, s)| m == "POST" && u.ends_with("/orders") && *s == 400));
    println!("create-order matched requests: {create:?}");
    println!("invalid-quantity matched requests: {invalid:?}");

    let baseline = anvil(root, &["qa", "baseline", "--start"]);
    assert!(baseline.status.success(), "{}", String::from_utf8_lossy(&baseline.stdout));
    assert!(root.join(".harness/qa/baseline/create-order/after-create.png").is_file());
    assert!(root.join(".harness/qa/baseline/invalid-quantity/error-banner.png").is_file());

    let (code, run, text) = run_json(root, &["qa", "run", "--all-flows", "--start", "--json"]);
    assert_eq!(code, 0, "second run failed: {text}");
    for id in ["create-order", "invalid-quantity"] {
        let result = &flow(&run, id)["screenshots"][0]["baseline"];
        assert_eq!(result["result"], "match", "{id}: {result}");
    }

    let path = root.join(".harness/qa/flows/create-order.json");
    let original = fs::read_to_string(&path).expect("read flow");
    fs::write(&path, original.replace("\"status\": 201", "\"status\": 200")).expect("break flow");
    let broken = anvil(root, &["qa", "run", "--flow", "create-order", "--start"]);
    let out = String::from_utf8_lossy(&broken.stdout).into_owned();
    println!("{out}");
    assert_eq!(broken.status.code(), Some(1));
    assert!(out.contains("expected status 200 but saw POST"), "{out}");
    fs::write(&path, original).expect("restore flow");
}
