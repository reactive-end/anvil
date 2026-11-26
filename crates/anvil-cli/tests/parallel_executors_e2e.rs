//! Orchestrator mode with several executors at once: each edits only its own brief's scope,
//! and a brief that overlaps a running or starting executor is refused.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

const SESSION: &str = "sess-1";

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
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/core.ts"), "export const x = 1;\n").unwrap();
        anvil(root, &["init"], None);
        anvil(root, &["orchestrator", "on"], None);
        let transcripts = TempDir::new().unwrap();
        fs::create_dir_all(transcripts.path().join(SESSION).join("subagents")).unwrap();
        Self { repo, transcripts }
    }

    fn main_transcript(&self) -> PathBuf {
        self.transcripts.path().join(format!("{SESSION}.jsonl"))
    }

    fn subagents(&self) -> PathBuf {
        self.transcripts.path().join(SESSION).join("subagents")
    }

    /// Writes what Claude Code keeps for a running or finished executor and its briefs.
    fn executor(&self, id: &str, call: &str, briefs: &[&str], finished: bool) {
        let meta =
            json!({ "agentType": "anvil-executor", "toolUseId": call, "finished": finished });
        fs::write(self.subagents().join(format!("agent-{id}.meta.json")), meta.to_string())
            .unwrap();
        let lines: Vec<String> = briefs
            .iter()
            .map(|b| {
                json!({ "type": "user", "message": { "role": "user", "content": b } }).to_string()
            })
            .collect();
        fs::write(self.subagents().join(format!("agent-{id}.jsonl")), lines.join("\n")).unwrap();
    }

    /// The permission decision for a tool call by `agent` (`None` is the orchestrator).
    fn decide(&self, agent: Option<&str>, call: &str, tool: &str, input: Value) -> String {
        let root = self.repo.path();
        let mut payload = json!({
            "hook_event_name": "PreToolUse",
            "session_id": SESSION,
            "transcript_path": self.main_transcript().to_string_lossy(),
            "cwd": root.to_string_lossy(),
            "tool_name": tool,
            "tool_use_id": call,
            "tool_input": input,
        });
        if let Some(id) = agent {
            payload["agent_id"] = json!(id);
            payload["agent_type"] = json!("anvil-executor");
        }
        let out =
            anvil(root, &["hook", "PreToolUse", "--adapter", "claude"], Some(&payload.to_string()));
        let parsed: Value = serde_json::from_slice(&out.stdout).unwrap();
        parsed["hookSpecificOutput"]["permissionDecision"].as_str().unwrap_or("none").to_owned()
    }

    fn edit(&self, agent: &str, rel: &str) -> String {
        let path = self.repo.path().join(rel);
        self.decide(
            Some(agent),
            "toolu_edit",
            "Edit",
            json!({ "file_path": path.to_string_lossy() }),
        )
    }

    fn spawn(&self, call: &str, brief: &str) -> String {
        self.decide(
            None,
            call,
            "Agent",
            json!({ "subagent_type": "anvil-executor", "prompt": brief }),
        )
    }

    fn brief_again(&self, to: &str, brief: &str) -> String {
        self.decide(None, "toolu_msg", "SendMessage", json!({ "to": to, "message": brief }))
    }
}

#[test]
fn each_executor_edits_only_its_own_latest_brief() {
    let s = Session::new();
    assert_eq!(s.spawn("toolu_a", "Auth\nSCOPE: src/auth"), "none");
    assert_eq!(s.spawn("toolu_u", "UI\nSCOPE: src/ui"), "none");
    s.executor("auth", "toolu_a", &["Auth\nSCOPE: src/auth"], false);
    s.executor("ui", "toolu_u", &["UI\nSCOPE: src/ui"], false);

    assert_eq!(s.edit("auth", "src/auth/login.ts"), "none");
    assert_eq!(
        s.edit("auth", "src/ui/button.ts"),
        "deny",
        "the session's other scope is not this executor's"
    );
    assert_eq!(s.edit("ui", "src/ui/button.ts"), "none");

    s.executor(
        "auth",
        "toolu_a",
        &["Auth\nSCOPE: src/auth", "Next step\nSCOPE: src/core.ts"],
        false,
    );
    assert_eq!(s.edit("auth", "src/core.ts"), "none");
    assert_eq!(s.edit("auth", "src/auth/login.ts"), "deny", "a new brief replaces the old scope");
}

#[test]
fn briefs_that_overlap_a_running_or_starting_executor_are_refused() {
    let s = Session::new();
    s.executor("auth", "toolu_a", &["Auth\nSCOPE: src/auth"], false);

    assert_eq!(s.spawn("toolu_1", "More auth\nSCOPE: src/auth/token.ts"), "deny");
    assert_eq!(s.spawn("toolu_2", "Database\nSCOPE: src/db"), "none");
    assert_eq!(
        s.spawn("toolu_3", "Queries\nSCOPE: src/db/query.ts"),
        "deny",
        "toolu_2 is still starting"
    );
    assert_eq!(s.spawn("toolu_4", "Docs\nSCOPE: docs"), "none");

    assert_eq!(
        s.brief_again("auth", "Continue\nSCOPE: src/auth/session.ts"),
        "none",
        "its own scope is not a clash"
    );
    assert_eq!(s.brief_again("auth", "Also\nSCOPE: src/db/pool.ts"), "deny");

    s.executor("auth", "toolu_a", &["Auth\nSCOPE: src/auth"], true);
    assert_eq!(
        s.spawn("toolu_5", "Auth again\nSCOPE: src/auth"),
        "none",
        "a finished executor frees its files"
    );
}

#[test]
fn check_scope_judges_only_one_executors_files() {
    let s = Session::new();
    let root = s.repo.path();
    fs::create_dir_all(root.join("src/auth")).unwrap();
    fs::create_dir_all(root.join("src/ui")).unwrap();
    fs::write(root.join("src/auth/login.ts"), "// @ts-ignore\nexport const a: number = 1;\n")
        .unwrap();
    fs::write(root.join("src/ui/button.ts"), "export const b: number = 2;\n").unwrap();
    let check = |scope: &str| {
        Command::new(env!("CARGO_BIN_EXE_anvil"))
            .arg("--root")
            .arg(root)
            .args(["check", "--tier", "0", "--scope", scope])
            .output()
            .unwrap()
    };
    let ui = check("src/ui");
    assert!(ui.status.success(), "{}", String::from_utf8_lossy(&ui.stdout));
    let auth = check("src/auth,src/ui");
    assert!(!auth.status.success(), "the escape hatch in src/auth is in scope");
    assert!(String::from_utf8_lossy(&auth.stdout).contains("src/auth/login.ts"));
}
