//! E2E tests for umbrella/monorepo `init`, opt-in SonarQube & anti-test-tampering, checkpoints,
//! worktrees, specs, trajectories, `mcp --proxy`, and task import/export/verify/groups.

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

fn run_anvil_stdin_env(cwd: &Path, args: &[&str], stdin_data: &str, envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(anvil_bin());
    cmd.args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env("ANVIL_NO_AUTO_DAEMON", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn anvil");
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
    Command::new("git").args(["config", "user.email", "dev@example.com"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.name", "Dev"]).current_dir(root).output().unwrap();
    fs::write(root.join("package.json"), r#"{"name":"p5-demo","version":"1.0.0"}"#).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/app.ts"), "export const v = 1;\n").unwrap();
    Command::new("git").args(["add", "-A"]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "init"]).current_dir(root).output().unwrap();
    dir
}

#[test]
fn init_thin_umbrella_layout_avoids_root_state_and_sets_subrepo_mcp() {
    let ws = TempDir::new().unwrap();
    let root = ws.path();
    fs::create_dir_all(root.join("backend/src")).unwrap();
    fs::create_dir_all(root.join("frontend/src")).unwrap();
    fs::write(root.join("backend/package.json"), r#"{"name":"be","version":"1.0.0"}"#).unwrap();
    fs::write(root.join("backend/sonar-project.properties"), "sonar.projectKey=be-key\n").unwrap();
    fs::write(
        root.join("frontend/package.json"),
        r#"{"name":"fe","dependencies":{"react":"18.0.0"}}"#,
    )
    .unwrap();

    // Prompts: layout [2], backend Sonar [y -> default URL], frontend Sonar [n].
    let out = run_anvil_stdin_env(root, &["init"], "2\ny\n\nn\n", &[("ANVIL_INIT_FORCE_PROMPT", "1")]);
    let stdout = assert_ok(&out, "interactive thin umbrella init");
    assert!(stdout.contains("How should Anvil manage this workspace?"));
    assert!(stdout.contains("Does subproject `backend` use SonarQube?"));
    assert!(stdout.contains("Does subproject `frontend` use SonarQube?"));
    assert!(!root.join(".harness").exists(), "thin umbrella must NOT create root .harness/");
    assert!(!root.join("tasks.json").exists(), "thin umbrella must NOT create root tasks.json");
    assert!(root.join("AGENTS.md").is_file());

    let mcp_cfg: Value = serde_json::from_str(&fs::read_to_string(root.join(".agents/mcp_config.json")).unwrap()).unwrap();
    assert!(mcp_cfg["mcpServers"]["anvil-backend"].is_object());
    assert!(mcp_cfg["mcpServers"]["anvil-frontend"].is_object());

    let be_cfg: Value = serde_json::from_str(&fs::read_to_string(root.join("backend/.harness/config.json")).unwrap()).unwrap();
    let fe_cfg: Value = serde_json::from_str(&fs::read_to_string(root.join("frontend/.harness/config.json")).unwrap()).unwrap();
    assert_eq!(be_cfg["sonar"]["enabled"], json!(true));
    assert_eq!(be_cfg["sonar"]["project_key"], json!("be-key"));
    assert_eq!(fe_cfg["sonar"]["enabled"], json!(false));
}

#[test]
fn init_monorepo_layout_keeps_single_harness_at_root() {
    let mono = TempDir::new().unwrap();
    let mroot = mono.path();
    fs::create_dir_all(mroot.join("backend/src")).unwrap();
    fs::create_dir_all(mroot.join("frontend/src")).unwrap();
    fs::write(mroot.join("backend/package.json"), r#"{"name":"be","version":"1.0.0"}"#).unwrap();
    fs::write(mroot.join("backend/sonar-project.properties"), "sonar.projectKey=be-mono\n").unwrap();
    fs::write(
        mroot.join("frontend/package.json"),
        r#"{"name":"fe","dependencies":{"react":"18.0.0"}}"#,
    )
    .unwrap();

    let mono_out = run_anvil_stdin_env(mroot, &["init"], "1\nn\ny\n\nn\n", &[("ANVIL_INIT_FORCE_PROMPT", "1")]);
    assert_ok(&mono_out, "interactive monorepo init");
    assert!(mroot.join(".harness/config.json").is_file());
    assert!(mroot.join("tasks.json").is_file());
    assert!(!mroot.join("backend/.harness").exists());
    assert!(!mroot.join("frontend/.harness").exists());
    let root_cfg: Value = serde_json::from_str(&fs::read_to_string(mroot.join(".harness/config.json")).unwrap()).unwrap();
    let be_sub = root_cfg["subrepos"].as_array().unwrap().iter().find(|s| s["path"] == "backend").unwrap();
    assert_eq!(be_sub["sonar_enabled"], json!(true));
}

#[test]
fn check_enforces_opt_in_sonar_complexity_and_anti_test_tampering() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    // Deeply nested function exceeds default Sonar limit=15 ONLY when --sonar is passed.
    let complex = r#"export function complex(a: boolean, b: boolean, c: boolean, d: boolean): number {
  if (a) {
    for (let i = 0; i < 3; i++) {
      if (b && c) {
        while (i > 0) {
          if (d || a) {
            if (c) {
              return 1;
            }
          }
        }
      }
    }
  }
  return 0;
}
"#;
    fs::write(root.join("src/app.ts"), complex).unwrap();
    assert_ok(&run_anvil(root, &["check", "--tier", "0"]), "default check skips sonar");
    let sonar_fail = run_anvil(root, &["check", "--tier", "0", "--sonar"]);
    assert!(!sonar_fail.status.success(), "--sonar must fail on high cognitive complexity");
    assert_ok(&run_anvil(root, &["check", "--tier", "0", "--no-sonar"]), "--no-sonar skips check");

    // Restoring src/app.ts and tampering with a committed test file must fail Tier 0.
    fs::write(root.join("src/app.ts"), "export const v = 1;\n").unwrap();
    fs::write(root.join("src/app.spec.ts"), "expect(1).toBe(1);\n").unwrap();
    Command::new("git").args(["add", "src/app.spec.ts"]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "add spec"]).current_dir(root).output().unwrap();

    let tampered = concat!("it", ".skip('bypass', () => {});\n");
    fs::write(root.join("src/app.spec.ts"), tampered).unwrap();
    let tamper_fail = run_anvil(root, &["check", "--tier", "0"]);
    assert!(!tamper_fail.status.success(), "no-test-tampering must block skipped tests");
}

#[test]
fn checkpoints_worktrees_specs_and_trajectory_stats_work() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    // Checkpoint save -> mutate & create new file -> rewind removes new file -> drop.
    let mut gi = fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
    gi.push_str(".env\n");
    fs::write(root.join(".gitignore"), gi).unwrap();
    assert_ok(&run_anvil(root, &["checkpoint", "save", "--label", "good"]), "ckpt save");
    fs::write(root.join("src/app.ts"), "export const v = 999;\n").unwrap();
    fs::write(root.join("src/extra_after_ckpt.ts"), "export const extra = 1;\n").unwrap();
    fs::write(root.join(".env"), "SECRET=keep_me\n").unwrap();
    let ckpt_list = assert_ok(&run_anvil(root, &["checkpoint", "list"]), "ckpt list");
    assert!(ckpt_list.contains("good"));
    assert_ok(&run_anvil(root, &["checkpoint", "rewind"]), "ckpt rewind");
    assert_eq!(fs::read_to_string(root.join("src/app.ts")).unwrap().trim(), "export const v = 1;");
    assert!(
        !root.join("src/extra_after_ckpt.ts").exists(),
        "checkpoint rewind must delete newly created files added after snapshot"
    );
    assert!(root.join(".env").is_file(), "checkpoint rewind must preserve ignored .env");
    assert_ok(&run_anvil(root, &["checkpoint", "drop", "all"]), "ckpt drop all");

    // Worktree add -> list -> remove.
    assert_ok(&run_anvil(root, &["worktree", "add", "agent-a"]), "wt add");
    assert!(root.join(".harness/.worktrees/agent-a/src/app.ts").is_file());
    assert_ok(&run_anvil(root, &["worktree", "remove", "agent-a", "--force"]), "wt remove");

    // Spec init -> update -> check.
    assert_ok(&run_anvil(root, &["spec", "init", "app-contract"]), "spec init");
    let spec_body = "# Spec: app\n\n## Files\n- src/app.ts\n\n## Must Contain\n- src/app.ts :: export const v\n\n## Forbid\n- src/app.ts :: panic!\n";
    fs::write(root.join(".harness/specs/app-contract.md"), spec_body).unwrap();
    assert_ok(&run_anvil(root, &["spec", "check"]), "spec check");

    // Code exec + trajectories stats.
    let code_out = assert_ok(
        &run_anvil(root, &["code", "exec", "return anvil.graph_query({ term: 'app', limit: 3 }).count;"]),
        "code exec",
    );
    let parsed: Value = serde_json::from_str(&code_out).unwrap();
    assert!(parsed["calls_executed"].as_u64().unwrap() >= 1);
    let stats_out = assert_ok(&run_anvil(root, &["stats", "--trajectories"]), "stats trajectories");
    assert!(stats_out.contains("p50="));
    assert!(stats_out.contains("code_mode:"));
}

#[test]
fn mcp_proxy_wraps_upstream_mcp_server_with_search_exec_and_mutation_guard() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    let proxy_cfg = json!({
        "servers": {
            "local_anvil": {
                "command": anvil_bin(),
                "args": ["--root", root.to_string_lossy(), "mcp"],
                "allow_mutations": false
            }
        }
    });
    fs::write(
        root.join(".harness/mcp-proxy.json"),
        serde_json::to_string_pretty(&proxy_cfg).unwrap(),
    )
    .unwrap();

    let session = [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "proxy_search", "arguments": { "query": "graph" } } }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "proxy_exec", "arguments": { "script": "return proxy.call('local_anvil', 'graph_query', { term: 'app', limit: 2 }).count;" } } }),
        json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "proxy_exec", "arguments": { "script": "return proxy.call('local_anvil', 'task_record', { title: 'x', summary: 'y' });" } } }),
    ]
    .iter()
    .map(|v| serde_json::to_string(v).unwrap())
    .collect::<Vec<_>>()
    .join("\n")
        + "\n";

    let out = run_anvil_stdin_env(root, &["mcp", "--proxy"], &session, &[]);
    let stdout = assert_ok(&out, "mcp --proxy");
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 2);
    assert!(lines[2]["result"]["content"][0]["text"].as_str().unwrap().contains("graph_query"));
    assert_eq!(lines[3]["result"]["isError"], json!(false));
    assert_eq!(lines[4]["result"]["isError"], json!(true));
}

#[test]
fn tasks_import_export_verify_and_status_filtering_work_end_to_end() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    let json_path = root.join("voz_features.json");
    fs::write(
        &json_path,
        r#"{
          "project": "voz",
          "updated": "2026-11-24",
          "features": [
            {
              "id": "F001",
              "name": "VOZ-RF-01 Crear bucket",
              "behavior": "Crear bucket",
              "verify": "echo verify_ok",
              "status": "not_started",
              "evidence": "En codigo"
            },
            {
              "id": "F003",
              "name": "VOZ-RF-03 VAD 512",
              "behavior": "Detectar voz",
              "verify": null,
              "status": "not_started",
              "evidence": "Pendiente"
            }
          ]
        }"#,
    )
    .unwrap();

    let imp = assert_ok(&run_anvil(root, &["tasks", "import", "voz_features.json"]), "import");
    assert!(imp.contains("imported=2"));
    let listed = assert_ok(&run_anvil(root, &["tasks", "list", "--status", "not_started"]), "list");
    assert!(listed.contains("F001 [not_started] VOZ-RF-01 Crear bucket [verify]"));
    assert!(listed.contains("F003 [not_started] VOZ-RF-03 VAD 512"));

    let verified = assert_ok(&run_anvil(root, &["tasks", "verify", "F001", "--record"]), "verify");
    assert!(verified.contains("PASS F001"));
    let exported = assert_ok(&run_anvil(root, &["tasks", "export", "--project", "voz"]), "export");
    let parsed: Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(parsed["features"][0]["status"], json!("done"));
}

#[test]
fn grouped_task_imports_keep_identical_ids_isolated() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    fs::write(
        root.join("avatar_features.json"),
        r#"{"project":"avatar","features":[{"id":"F001","name":"AVATAR F1","verify":"echo av_ok","status":"not_started","evidence":"dev"}]}"#,
    )
    .unwrap();
    fs::write(
        root.join("llm_features.json"),
        r#"{"project":"llm","features":[{"id":"F001","name":"LLM F1","verify":null,"status":"not_started","evidence":"dev"}]}"#,
    )
    .unwrap();
    assert_ok(&run_anvil(root, &["tasks", "import", "avatar_features.json", "--group", "avatar"]), "import avatar");
    assert_ok(&run_anvil(root, &["tasks", "import", "llm_features.json", "--group", "llm"]), "import llm");

    let avatar_list = assert_ok(&run_anvil(root, &["tasks", "list", "--group", "avatar"]), "list avatar");
    assert!(avatar_list.contains("avatar/F001 [not_started] AVATAR F1 [verify]"));
    assert!(!avatar_list.contains("llm/F001"));

    let verify_grp = assert_ok(&run_anvil(root, &["tasks", "verify", "avatar/F001", "--record"]), "verify avatar");
    assert!(verify_grp.contains("PASS avatar/F001"));
    let exp_av = assert_ok(&run_anvil(root, &["tasks", "export", "--group", "avatar"]), "export avatar");
    let av_val: Value = serde_json::from_str(&exp_av).unwrap();
    assert_eq!(av_val["features"][0]["id"], json!("F001"));
    assert_eq!(av_val["features"][0]["status"], json!("done"));
}

#[test]
fn task_progress_resume_and_session_start_lifecycle_e2e() {
    let repo = setup_git_ts_repo();
    let root = repo.path();
    assert_ok(&run_anvil(root, &["init"]), "init");

    fs::write(
        root.join("core_features.json"),
        r#"{"project":"core","features":[{"id":"F001","name":"Core discount rule","behavior":"Add volume discount","verify":"echo discount_ok","status":"not_started"}]}"#,
    )
    .unwrap();
    assert_ok(&run_anvil(root, &["tasks", "import", "core_features.json", "--group", "core"]), "import");
    let started = assert_ok(&run_anvil(root, &["tasks", "start", "core/F001"]), "start existing");
    assert!(started.contains("started existing task core/F001"));
    assert!(root.join(".harness/tasks/core/F001/progress.md").is_file());

    let updated = assert_ok(
        &run_anvil(
            root,
            &[
                "tasks",
                "update",
                "core/F001",
                "Added tier calc",
                "--done",
                "Added discount helper",
                "--next",
                "Wire subtotal,Run verify",
                "--files",
                "src/app.ts",
            ],
        ),
        "update progress",
    );
    assert!(updated.contains("progress=.harness/tasks/core/F001/progress.md"));

    let hook_out = assert_ok(
        &run_anvil_stdin_env(root, &["hook", "SessionStart", "--adapter", "antigravity"], "{}", &[]),
        "session start",
    );
    assert!(hook_out.contains("core/F001"));
    assert!(hook_out.contains("Wire subtotal"));

    let resumed = assert_ok(&run_anvil(root, &["tasks", "resume"]), "resume");
    assert!(resumed.contains("core/F001"));
    assert!(resumed.contains("Added discount helper"));
    assert!(resumed.contains("Wire subtotal; Run verify"));

    let verified = assert_ok(&run_anvil(root, &["tasks", "verify", "core/F001", "--record"]), "verify");
    assert!(verified.contains("PASS core/F001"));
}
