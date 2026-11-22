use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
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

fn setup_git_ts_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    Command::new("git").args(["init"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.name", "Test User"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.email", "test@example.com"]).current_dir(root).output().unwrap();

    fs::write(root.join("package.json"), r#"{"name":"sample","version":"1.0.0"}"#).unwrap();
    fs::write(root.join("tsconfig.json"), r#"{"compilerOptions":{"strict":true}}"#).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/math.ts"),
        "import { tag } from './tag';\nexport function add(a: number, b: number): number {\n  return a + b;\n}\nexport function sub(a: number, b: number): number {\n  return a - b;\n}\n",
    )
    .unwrap();
    fs::write(root.join("src/tag.ts"), "export const tag = 'v3';\n").unwrap();
    fs::write(
        root.join("src/app.ts"),
        "import { add } from './math';\nexport function runApp(): number {\n  return add(2, 3);\n}\n",
    )
    .unwrap();

    Command::new("git").args(["add", "-A"]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "initial"]).current_dir(root).output().unwrap();
    dir
}

struct DaemonGuard(PathBuf);

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = Command::new(anvil_bin())
            .arg("--root")
            .arg(&self.0)
            .args(["daemon", "stop"])
            .output();
    }
}

#[test]
fn graph_outline_and_read_symbol_return_accurate_slices_and_savings() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    let outline_out = assert_ok(
        &run_anvil(root, &["graph", "outline", "src/math.ts", "--max-tokens", "1000"]),
        "graph outline",
    );
    assert!(outline_out.contains("src/math.ts:"));
    assert!(outline_out.contains("add"));
    assert!(outline_out.contains("sub"));
    assert!(outline_out.contains("tokens_saved="));

    let sym_out = assert_ok(
        &run_anvil(root, &["graph", "read-symbol", "add"]),
        "graph read-symbol",
    );
    assert!(sym_out.contains("add (function) src/math.ts:"));
    assert!(sym_out.contains("import { tag } from './tag';"));
    assert!(sym_out.contains("return a + b;"));
    assert!(!sym_out.contains("return a - b;"));
}

#[test]
fn anvil_check_tier1_hits_cache_on_second_run() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");
    Command::new("git").args(["add", "-A"]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "init harness"]).current_dir(root).output().unwrap();

    // Modify a TS source file so minimum_tier == Tier1 and changed is non-empty.
    fs::write(
        root.join("src/math.ts"),
        "import { tag } from './tag';\nexport function add(a: number, b: number): number {\n  return a + b + 0;\n}\nexport function sub(a: number, b: number): number {\n  return a - b;\n}\n",
    )
    .unwrap();

    let first = assert_ok(&run_anvil(root, &["check", "--no-tests"]), "first check");
    assert!(!first.contains("PASS (cached)"), "first run should not hit cache: {first}");

    let second = assert_ok(&run_anvil(root, &["check", "--no-tests"]), "second check");
    assert!(second.contains("[Tier 1] PASS (cached)"), "expected cache hit, got: {second}");

    let bypass = assert_ok(
        &run_anvil(root, &["check", "--no-tests", "--no-cache"]),
        "check --no-cache",
    );
    assert!(!bypass.contains("PASS (cached)"), "--no-cache must bypass cache: {bypass}");
}

#[test]
fn anvil_diff_strips_lockfile_noise_and_grep_returns_bounded_matches() {
    let repo = setup_git_ts_repo();
    let root = repo.path();

    fs::write(root.join("package-lock.json"), "{\"lockfileVersion\":3}\n").unwrap();
    Command::new("git").args(["add", "-A"]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "add lockfile"]).current_dir(root).output().unwrap();

    assert_ok(&run_anvil(root, &["init"]), "init");

    // Modify both package-lock.json and src/math.ts.
    fs::write(
        root.join("package-lock.json"),
        "{\"lockfileVersion\":3,\"packages\":{\"a\":{\"version\":\"1.0.0\"}}}\n",
    )
    .unwrap();
    fs::write(
        root.join("src/math.ts"),
        "import { tag } from './tag';\nexport function add(a: number, b: number): number {\n  return a + b + 1;\n}\nexport function sub(a: number, b: number): number {\n  return a - b;\n}\n",
    )
    .unwrap();

    let diff_out = assert_ok(&run_anvil(root, &["diff", "--max-lines", "80"]), "anvil diff");
    assert!(diff_out.contains("skipped 1 noisy file(s): package-lock.json"), "got: {diff_out}");
    assert!(diff_out.contains("# affected_symbols: src/math.ts"), "got: {diff_out}");
    assert!(diff_out.contains("return a + b + 1;"));
    assert!(!diff_out.contains("lockfileVersion"));

    let grep_out = assert_ok(
        &run_anvil(root, &["grep", "return a [+-]", "--max-matches", "1", "--max-tokens", "200"]),
        "anvil grep",
    );
    assert!(grep_out.contains("src/math.ts:"));
    assert!(grep_out.contains("1/2 match(es)"));
    assert!(grep_out.contains("truncated=true"));
}

#[test]
fn pre_tool_use_token_guard_blocks_unbounded_noise_read_and_warns_heavy_file() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    fs::write(root.join("package-lock.json"), "{}\n").unwrap();
    let unbounded_noise = json!({
        "hookEventName": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "toolCall": {
            "toolName": "read_file",
            "toolInput": { "Path": "package-lock.json", "path": "package-lock.json" }
        }
    });
    let deny_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-tool-use", "--adapter", "antigravity"], &unbounded_noise.to_string()),
        "token guard deny unbounded lockfile",
    );
    let deny_json: Value = serde_json::from_str(&deny_out).unwrap();
    assert_eq!(deny_json["decision"], json!("deny"));
    assert!(deny_json["reason"].as_str().unwrap().contains("package-lock.json"));

    let bounded_noise = json!({
        "hookEventName": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "toolCall": {
            "toolName": "read_file",
            "toolInput": { "path": "package-lock.json", "StartLine": 1, "EndLine": 20 }
        }
    });
    let allow_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-tool-use", "--adapter", "antigravity"], &bounded_noise.to_string()),
        "token guard allow bounded lockfile",
    );
    assert_eq!(serde_json::from_str::<Value>(&allow_out).unwrap()["decision"], json!("allow"));

    // > 800-line source file is allowed with a queued advisory InboxMessage.
    let heavy = (0..820).map(|i| format!("export const v{i} = {i};")).collect::<Vec<_>>().join("\n");
    fs::write(root.join("src/heavy.ts"), heavy).unwrap();
    let heavy_read = json!({
        "hookEventName": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "toolCall": {
            "toolName": "view_file",
            "toolInput": { "AbsolutePath": root.join("src/heavy.ts").to_string_lossy() }
        }
    });
    let heavy_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-tool-use", "--adapter", "antigravity"], &heavy_read.to_string()),
        "token guard heavy file allow + warn",
    );
    assert_eq!(serde_json::from_str::<Value>(&heavy_out).unwrap()["decision"], json!("allow"));

    let pre_inv = json!({ "hookEventName": "PreInvocation", "cwd": root.to_string_lossy() });
    let inv_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-invocation", "--adapter", "antigravity"], &pre_inv.to_string()),
        "drain heavy file warning",
    );
    assert!(inv_out.contains("token-guard"));
    assert!(inv_out.contains("800 lines"));
}

#[test]
fn daemon_background_start_status_query_and_stop_roundtrip() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");
    let _guard = DaemonGuard(root.to_path_buf());

    let start_out = assert_ok(&run_anvil(root, &["daemon", "start", "--background"]), "daemon start");
    assert!(start_out.contains("daemon started:"));
    assert!(root.join(".harness/.state/daemon.json").is_file());

    let status_out = assert_ok(&run_anvil(root, &["daemon", "status"]), "daemon status");
    assert!(status_out.contains("daemon: running"));

    let query_out = assert_ok(&run_anvil(root, &["graph", "query", "runApp"]), "daemon graph query");
    assert!(query_out.contains("runApp"));

    let sym_out = assert_ok(&run_anvil(root, &["graph", "read-symbol", "add"]), "daemon read-symbol");
    assert!(sym_out.contains("return a + b;"));

    let stop_out = assert_ok(&run_anvil(root, &["daemon", "stop"]), "daemon stop");
    assert!(stop_out.contains("daemon stopped"));
    assert!(!root.join(".harness/.state/daemon.json").exists());

    let stopped_status = assert_ok(&run_anvil(root, &["daemon", "status"]), "daemon stopped status");
    assert!(stopped_status.contains("daemon: stopped"));
}

#[test]
fn anvil_stats_reports_recorded_metrics_and_reset_clears_them() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    assert_ok(&run_anvil(root, &["graph", "outline", "src/math.ts"]), "outline stats");
    assert_ok(&run_anvil(root, &["graph", "read-symbol", "add"]), "read-symbol stats");

    let stats_out = assert_ok(&run_anvil(root, &["stats"]), "anvil stats");
    assert!(stats_out.contains("anvil stats:"));
    assert!(stats_out.contains("tokens_saved:"));
    assert!(stats_out.contains("symbol_read="));
    assert!(stats_out.contains("outline="));

    let reset_out = assert_ok(&run_anvil(root, &["stats", "--reset"]), "anvil stats --reset");
    assert!(reset_out.contains("stats reset"));

    let after_reset = assert_ok(&run_anvil(root, &["stats"]), "anvil stats after reset");
    assert!(after_reset.contains("total=0 (compressor=0, symbol_read=0, outline=0)"));
}
