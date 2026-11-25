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

    // Wide bounded read on a noise file (> 200 lines) is also blocked.
    let wide_noise = json!({
        "hookEventName": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "toolCall": {
            "toolName": "read_file",
            "toolInput": { "path": "package-lock.json", "StartLine": 1, "EndLine": 50000 }
        }
    });
    let wide_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-tool-use", "--adapter", "antigravity"], &wide_noise.to_string()),
        "token guard deny wide noise read",
    );
    assert_eq!(serde_json::from_str::<Value>(&wide_out).unwrap()["decision"], json!("deny"));
}

#[test]
fn pre_tool_use_token_guard_blocks_heavy_source_and_allows_bounded_slice() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    // > 800-line unbounded source file is denied with a compact graph outline suggestion.
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
        "token guard deny unbounded heavy file",
    );
    let heavy_json: Value = serde_json::from_str(&heavy_out).unwrap();
    assert_eq!(heavy_json["decision"], json!("deny"));
    assert!(heavy_json["reason"].as_str().unwrap_or("").contains("800 lines"));

    // Bounded <= 400-line slice on the same heavy file is allowed.
    let bounded_heavy = json!({
        "hookEventName": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "toolCall": {
            "toolName": "view_file",
            "toolInput": {
                "AbsolutePath": root.join("src/heavy.ts").to_string_lossy(),
                "StartLine": 1,
                "EndLine": 40
            }
        }
    });
    let bounded_heavy_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-tool-use", "--adapter", "antigravity"], &bounded_heavy.to_string()),
        "token guard allow bounded heavy file",
    );
    assert_eq!(serde_json::from_str::<Value>(&bounded_heavy_out).unwrap()["decision"], json!("allow"));
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

fn claude_read_decision(root: &Path, tool_input: Value) -> (String, String) {
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "session_id": "repro-guard",
        "cwd": root.to_string_lossy(),
        "tool_name": "Read",
        "tool_input": tool_input
    });
    let out = assert_ok(
        &run_anvil_stdin(root, &["hook", "PreToolUse", "--adapter", "claude"], &payload.to_string()),
        "claude PreToolUse Read",
    );
    let parsed: Value = serde_json::from_str(&out).unwrap();
    if parsed == json!({}) {
        return ("none".to_owned(), String::new());
    }
    let specific = &parsed["hookSpecificOutput"];
    (
        specific["permissionDecision"].as_str().unwrap().to_owned(),
        specific["permissionDecisionReason"].as_str().unwrap_or("").to_owned(),
    )
}

fn sample_png_bytes(newlines: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend(std::iter::repeat_n(b"\x00\n", newlines).flatten());
    bytes
}

fn read_abs(root: &Path, rel: &str) -> Value {
    json!({ "file_path": root.join(rel).to_string_lossy() })
}

#[test]
fn pre_tool_use_token_guard_allows_media_with_newlines_and_uses_claude_text_params() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    let png_900 = sample_png_bytes(900);
    fs::write(root.join("falso-900.png"), &png_900).unwrap();
    fs::write(root.join("falso-900.jpg"), [b"\xFF\xD8\xFF\xE0".as_slice(), &png_900[8..]].concat()).unwrap();
    let mut pdf_900 = b"%PDF-1.4\n".to_vec();
    pdf_900.extend(std::iter::repeat_n(b"\x00\n", 900).flatten());
    fs::write(root.join("falso-900.pdf"), &pdf_900).unwrap();

    assert_eq!(claude_read_decision(root, read_abs(root, "falso-900.png")).0, "none");
    assert_eq!(claude_read_decision(root, read_abs(root, "falso-900.jpg")).0, "none");
    assert_eq!(claude_read_decision(root, read_abs(root, "falso-900.pdf")).0, "none");
    assert_eq!(
        claude_read_decision(root, json!({ "file_path": root.join("falso-900.pdf").to_string_lossy(), "pages": "1-2" })).0,
        "none"
    );

    fs::write(root.join("texto-790.txt"), "x\n".repeat(790)).unwrap();
    fs::write(root.join("texto-900.txt"), "x\n".repeat(900)).unwrap();
    assert_eq!(claude_read_decision(root, read_abs(root, "texto-790.txt")).0, "none");
    let (txt_deny, txt_reason) = claude_read_decision(root, read_abs(root, "texto-900.txt"));
    assert_eq!(txt_deny, "deny");
    assert!(txt_reason.contains("offset"));
    assert!(txt_reason.contains("limit"));
    assert!(!txt_reason.contains("StartLine"));
    assert_eq!(
        claude_read_decision(root, json!({ "file_path": root.join("texto-900.txt").to_string_lossy(), "offset": 1, "limit": 100 })).0,
        "none"
    );
}

fn init_tight_binary_caps_repo(max_binary_bytes: u64, max_pdf_bytes: u64, max_pdf_pages: usize) -> TempDir {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    let cfg_path = root.join(".harness/config.json");
    let mut cfg: Value = serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg["token_guard"] = json!({
        "max_binary_bytes": max_binary_bytes,
        "max_pdf_bytes": max_pdf_bytes,
        "max_pdf_pages": max_pdf_pages
    });
    fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    repo
}

#[test]
fn pre_tool_use_token_guard_enforces_image_and_binary_byte_caps() {
    let repo = init_tight_binary_caps_repo(4096, 4096, 20);
    let root = repo.path();

    fs::write(root.join("grande-sin-saltos.png"), [b"\x89PNG".as_slice(), &vec![b'A'; 6000]].concat()).unwrap();
    let (img_deny, img_reason) = claude_read_decision(root, read_abs(root, "grande-sin-saltos.png"));
    assert_eq!(img_deny, "deny");
    assert!(img_reason.contains("image"));
    assert!(img_reason.contains("KB"));
    assert!(!img_reason.contains("StartLine"));
    assert!(!img_reason.contains("anvil grep"));

    fs::write(root.join("payload.bin"), [b"BIN\x00".as_slice(), &vec![0u8; 6000]].concat()).unwrap();
    let (bin_deny, bin_reason) = claude_read_decision(root, read_abs(root, "payload.bin"));
    assert_eq!(bin_deny, "deny");
    assert!(bin_reason.contains("binary file"));
}

#[test]
fn pre_tool_use_token_guard_enforces_pdf_byte_cap_and_pages_ranges() {
    let repo = init_tight_binary_caps_repo(4096, 4096, 20);
    let root = repo.path();

    fs::write(root.join("heavy.pdf"), [b"%PDF-1.7\n".as_slice(), &vec![b'B'; 6000]].concat()).unwrap();
    let (pdf_deny, pdf_reason) = claude_read_decision(root, read_abs(root, "heavy.pdf"));
    assert_eq!(pdf_deny, "deny");
    assert!(pdf_reason.contains("pages"));
    assert!(!pdf_reason.contains("StartLine"));
    assert!(!pdf_reason.contains("anvil grep"));
    assert_eq!(claude_read_decision(root, json!({ "file_path": root.join("heavy.pdf").to_string_lossy(), "offset": 1, "limit": 100 })).0, "deny");
    assert_eq!(claude_read_decision(root, json!({ "file_path": root.join("heavy.pdf").to_string_lossy(), "pages": "1-3" })).0, "none");
    let (pages_deny, pages_reason) = claude_read_decision(root, json!({ "file_path": root.join("heavy.pdf").to_string_lossy(), "pages": "1-25" }));
    assert_eq!(pages_deny, "deny");
    assert!(pages_reason.contains("25 pages > 20"));
}

#[test]
fn pre_tool_use_token_guard_allows_storage_media_while_blocking_noise() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    fs::create_dir_all(root.join("storage")).unwrap();
    fs::write(root.join("storage/chart.png"), sample_png_bytes(900)).unwrap();
    assert_eq!(claude_read_decision(root, read_abs(root, "storage/chart.png")).0, "none");

    fs::write(root.join("package-lock.json"), "{}\n").unwrap();
    fs::write(root.join("app.min.js"), "console.log(1);\n").unwrap();
    let (lock_deny, lock_reason) = claude_read_decision(root, read_abs(root, "package-lock.json"));
    assert_eq!(lock_deny, "deny");
    assert!(lock_reason.contains("offset"));
    assert!(!lock_reason.contains("StartLine"));
    assert_eq!(claude_read_decision(root, read_abs(root, "app.min.js")).0, "deny");
}

#[test]
fn pre_tool_use_token_guard_denial_does_not_leak_warning_into_post_tool_use() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    fs::write(root.join("texto-900.txt"), "x\n".repeat(900)).unwrap();
    let (decision, _) = claude_read_decision(
        root,
        json!({ "file_path": root.join("texto-900.txt").to_string_lossy() }),
    );
    assert_eq!(decision, "deny");

    // Subsequent unrelated PostToolUse (Bash) must NOT receive stale `exceeds 800 lines` warnings.
    let post_payload = json!({
        "hook_event_name": "PostToolUse",
        "session_id": "repro-guard",
        "cwd": root.to_string_lossy(),
        "tool_name": "Bash",
        "tool_input": { "command": "echo hi" },
        "tool_response": { "exitCode": 0, "stdout": "hi\n" }
    });
    let post_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "PostToolUse", "--adapter", "claude"], &post_payload.to_string()),
        "claude PostToolUse Bash after denied Read",
    );
    assert!(!post_out.contains("exceeds 800 lines"), "unexpected stale warning leak: {post_out}");
    assert_eq!(serde_json::from_str::<Value>(&post_out).unwrap(), json!({}));
}

#[test]
fn pre_tool_use_token_guard_allows_qa_screenshots_endline_only_and_external_dist() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    fs::create_dir_all(root.join(".harness/.state/qa")).unwrap();
    fs::write(root.join(".harness/.state/qa/before.png"), sample_png_bytes(900)).unwrap();
    let qa_payload = json!({
        "hookEventName": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "toolCall": {
            "toolName": "read_file",
            "toolInput": { "AbsolutePath": root.join(".harness/.state/qa/before.png").to_string_lossy() }
        }
    });
    let qa_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-tool-use", "--adapter", "antigravity"], &qa_payload.to_string()),
        "allow qa screenshot",
    );
    assert_eq!(serde_json::from_str::<Value>(&qa_out).unwrap()["decision"], json!("allow"));

    fs::write(root.join("src/heavy.ts"), "export const x = 1;\n".repeat(850)).unwrap();
    let endline_payload = json!({
        "hookEventName": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "toolCall": {
            "toolName": "read_file",
            "toolInput": { "path": "src/heavy.ts", "EndLine": 40 }
        }
    });
    let end_out = assert_ok(
        &run_anvil_stdin(root, &["hook", "pre-tool-use", "--adapter", "antigravity"], &endline_payload.to_string()),
        "allow EndLine-only read",
    );
    assert_eq!(serde_json::from_str::<Value>(&end_out).unwrap()["decision"], json!("allow"));

    let external_dir = TempDir::new().unwrap();
    let external_dist = external_dir.path().join("dist");
    fs::create_dir_all(&external_dist).unwrap();
    let external_file = external_dist.join("notes.ts");
    fs::write(&external_file, "export const ok = true;\n").unwrap();
    let ext_out = claude_read_decision(root, json!({ "file_path": external_file.to_string_lossy() }));
    assert_eq!(ext_out.0, "none");
}

#[test]
fn pre_tool_use_token_guard_covers_unbounded_shell_dumps() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");
    fs::write(root.join("heavy.txt"), "line\n".repeat(900)).unwrap();
    fs::write(root.join("light.txt"), "line\n".repeat(20)).unwrap();

    let decide = |cmd: &str| {
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "cwd": root.to_string_lossy(),
            "tool_name": "Bash",
            "tool_input": { "command": cmd }
        });
        let out = assert_ok(
            &run_anvil_stdin(root, &["hook", "PreToolUse", "--adapter", "claude"], &payload.to_string()),
            cmd,
        );
        let parsed: Value = serde_json::from_str(&out).unwrap();
        parsed["hookSpecificOutput"]["permissionDecision"].as_str().unwrap_or("none").to_owned()
    };

    for cmd in ["cat heavy.txt", "cd . && cat light.txt heavy.txt", "Get-Content heavy.txt", "cat < heavy.txt", "cat heavy.txt 2>/dev/null"] {
        assert_eq!(decide(cmd), "deny", "{cmd}");
    }
    for cmd in ["cat light.txt", "cat heavy.txt | wc -l", "cat heavy.txt > copy.txt", "sed -n '1,50p' heavy.txt", "Get-Content heavy.txt -TotalCount 20", "echo $(cat heavy.txt | head -1)"] {
        assert_eq!(decide(cmd), "none", "{cmd}");
    }

    let stats = assert_ok(&run_anvil(root, &["stats"]), "stats");
    assert!(stats.contains("blocks=5") && !stats.contains("tokens_avoided=0"), "{stats}");
    assert!(stats.contains("interventions:"), "{stats}");
}
