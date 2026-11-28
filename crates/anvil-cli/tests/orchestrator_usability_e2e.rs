//! Orchestrator mode as a user meets it: commits, help, built binaries, sandbox writes,
//! gitignored edits, and the Stop hook while an executor runs.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

const SESSION: &str = "sess-u";

fn anvil(repo: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_anvil"))
        .arg("--root")
        .arg(repo)
        .args(args)
        .env("ANVIL_LANG", "en")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn anvil");
    if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(data.as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    out
}

struct Session {
    repo: TempDir,
    transcripts: TempDir,
}

impl Session {
    fn new() -> Self {
        let repo = TempDir::new().unwrap();
        let root = repo.path();
        Command::new("git").arg("init").current_dir(root).output().unwrap();
        fs::write(root.join(".gitignore"), "/test/\n").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/core.ts"), "export const x = 1;\n").unwrap();
        anvil(root, &["init"], None);
        anvil(root, &["orchestrator", "on"], None);
        let transcripts = TempDir::new().unwrap();
        fs::create_dir_all(transcripts.path().join(SESSION).join("subagents")).unwrap();
        Self { repo, transcripts }
    }

    fn subagents(&self) -> PathBuf {
        self.transcripts.path().join(SESSION).join("subagents")
    }

    fn executor(&self, id: &str, brief: &str, finished: bool) {
        let meta =
            json!({ "agentType": "anvil-executor", "toolUseId": "toolu_a", "finished": finished });
        fs::write(self.subagents().join(format!("agent-{id}.meta.json")), meta.to_string())
            .unwrap();
        let line =
            json!({ "type": "user", "message": { "role": "user", "content": brief } }).to_string();
        fs::write(self.subagents().join(format!("agent-{id}.jsonl")), line).unwrap();
    }

    fn payload(&self, event: &str, agent: Option<&str>) -> Value {
        let transcript = self.transcripts.path().join(format!("{SESSION}.jsonl"));
        let mut payload = json!({
            "hook_event_name": event,
            "session_id": SESSION,
            "transcript_path": transcript.to_string_lossy(),
            "cwd": self.repo.path().to_string_lossy(),
        });
        if let Some(id) = agent {
            payload["agent_id"] = json!(id);
            payload["agent_type"] = json!("anvil-executor");
        }
        payload
    }

    fn hook(&self, event: &str, payload: &Value) -> Value {
        let out = anvil(
            self.repo.path(),
            &["hook", event, "--adapter", "claude"],
            Some(&payload.to_string()),
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// The permission decision for a shell command by `agent` (`None` is the orchestrator).
    fn shell(&self, agent: Option<&str>, command: &str) -> String {
        let mut payload = self.payload("PreToolUse", agent);
        payload["tool_name"] = json!("Bash");
        payload["tool_use_id"] = json!("toolu_sh");
        payload["tool_input"] = json!({ "command": command });
        let parsed = self.hook("PreToolUse", &payload);
        parsed["hookSpecificOutput"]["permissionDecision"].as_str().unwrap_or("none").to_owned()
    }

    fn stop(&self) -> Value {
        self.hook("Stop", &self.payload("Stop", None))
    }
}

#[test]
fn the_orchestrator_commits_and_publishes_but_does_not_apply_patches() {
    let s = Session::new();
    for ok in [
        "git add -A && git commit -m 'feat: x'",
        "git push origin main",
        "git switch -c topic",
        "gh pr create --fill",
        "gh run watch 12",
        "anvil sync --help",
        "anvil orchestrator off --help",
        "anvil help tasks done",
    ] {
        assert_eq!(s.shell(None, ok), "none", "orchestrator: {ok}");
    }
    for bad in ["git apply fix.patch", "git am fix.patch", "gh pr merge 3", "anvil sync"] {
        assert_eq!(s.shell(None, bad), "deny", "orchestrator: {bad}");
    }
}

#[test]
fn executors_may_read_help_but_never_commit() {
    let s = Session::new();
    s.executor("ex", "Work\nSCOPE: src", false);
    for ok in ["anvil sync --help", "anvil tasks done --help", "anvil orchestrator off -h"] {
        assert_eq!(s.shell(Some("ex"), ok), "none", "executor: {ok}");
    }
    for bad in ["git commit -m x", "git push", "anvil sync", "anvil orchestrator off"] {
        assert_eq!(s.shell(Some("ex"), bad), "deny", "executor: {bad}");
    }
}

#[test]
fn a_freshly_built_binary_can_test_the_guard() {
    let s = Session::new();
    let query = "\"aws --profile prod ec2 terminate-instances --instance-ids i-1\"";
    for exe in
        ["./target/debug/anvil.exe", "target/release/anvil", "C:\\path\\target\\debug\\anvil.exe"]
    {
        let cmd = format!("{exe} guard test {query}");
        assert_ne!(s.shell(None, &cmd), "deny", "orchestrator: {cmd}");
        assert_ne!(s.shell(Some("ex"), &cmd), "deny", "executor: {cmd}");
    }
}

#[test]
fn sandbox_work_inside_the_scope_is_not_refused() {
    let s = Session::new();
    fs::create_dir_all(s.repo.path().join("test/hexagonal-sandbox/orders-api")).unwrap();
    s.executor("ex", "Sandbox\nSCOPE: test/hexagonal-sandbox", false);
    for ok in [
        "New-Item -ItemType Directory -Path test/hexagonal-sandbox/orders-api/src",
        "npx playwright install chromium",
        "cd test/hexagonal-sandbox/orders-api && cp ../README.md x",
        "cd test/hexagonal-sandbox && Set-Content -Path a.txt -Value hi -Encoding utf8",
        "touch $SANDBOX/a.txt",
        "cd test/hexagonal-sandbox/orders-api/../.. && ls",
    ] {
        assert_eq!(s.shell(Some("ex"), ok), "none", "executor: {ok}");
    }
    for bad in [
        "cd test/hexagonal-sandbox/orders-api && cp ../../../README.md ../../../x",
        "cd test/hexagonal-sandbox && touch ../../src/core.ts",
        "New-Item -ItemType File -Path src/core.ts",
        "touch test/hexagonal-sandbox/../other/a",
    ] {
        assert_eq!(s.shell(Some("ex"), bad), "deny", "executor: {bad}");
    }
}

#[test]
fn edits_under_gitignored_paths_are_not_recorded_for_the_task() {
    let s = Session::new();
    let root = s.repo.path();
    fs::create_dir_all(root.join("test/sandbox")).unwrap();
    fs::write(root.join("test/sandbox/a.txt"), "x").unwrap();
    for file in ["test/sandbox/a.txt", "src/core.ts"] {
        let mut payload = s.payload("PostToolUse", None);
        payload["tool_name"] = json!("Write");
        payload["tool_input"] = json!({ "file_path": root.join(file).to_string_lossy() });
        s.hook("PostToolUse", &payload);
    }
    let recorded = fs::read_to_string(root.join(".harness/.state/untracked-edits.json")).unwrap();
    assert!(recorded.contains("src/core.ts"), "{recorded}");
    assert!(!recorded.contains("test/sandbox"), "{recorded}");
}

#[test]
fn stop_does_not_demand_task_done_while_an_executor_runs() {
    let s = Session::new();
    let root = s.repo.path();
    anvil(root, &["tasks", "start", "Implement the thing"], None);
    fs::write(root.join("src/core.ts"), "export const x = 2;\nexport const y = 3;\n").unwrap();
    fs::write(root.join("src/more.ts"), "export const z = 1;\n").unwrap();

    s.executor("ex", "Work\nSCOPE: src", true);
    let finished = s.stop().to_string();
    assert!(finished.contains("Task T-001 is still in progress"), "{finished}");

    s.executor("ex", "Work\nSCOPE: src", false);
    let running = s.stop().to_string();
    assert!(!running.contains("still in progress"), "{running}");
    assert!(running.contains("stays in progress while an executor is running"), "{running}");
}

impl Session {
    /// The permission decision for the main thread starting a subagent of `kind`.
    fn spawn(&self, kind: &str) -> String {
        let mut payload = self.payload("PreToolUse", None);
        payload["tool_name"] = json!("Agent");
        payload["tool_use_id"] = json!("toolu_spawn");
        payload["tool_input"] =
            json!({ "subagent_type": kind, "description": "x", "prompt": "Work\nSCOPE: src" });
        let parsed = self.hook("PreToolUse", &payload);
        parsed["hookSpecificOutput"]["permissionDecision"].as_str().unwrap_or("none").to_owned()
    }

    fn prompt_context(&self) -> String {
        let mut payload = self.payload("UserPromptSubmit", None);
        payload["prompt"] = json!("next step please");
        let parsed = self.hook("UserPromptSubmit", &payload);
        parsed["hookSpecificOutput"]["additionalContext"].as_str().unwrap_or_default().to_owned()
    }
}

#[test]
fn switching_the_mode_off_is_announced_once_and_stops_executor_spawns() {
    let s = Session::new();
    assert!(s.prompt_context().contains("[orchestrator-mode]: ON"));
    assert_eq!(s.spawn("anvil-executor"), "none");

    anvil(s.repo.path(), &["orchestrator", "off"], None);
    let first = s.prompt_context();
    assert!(first.contains("[orchestrator-mode]: OFF"), "{first}");
    assert!(!s.prompt_context().contains("orchestrator-mode"), "announced only once");

    assert_eq!(s.spawn("anvil-executor"), "deny");
    assert_eq!(s.spawn("Explore"), "none", "other subagents stay available");
}
