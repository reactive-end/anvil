//! The session's executors and their briefs, read from Claude Code's subagent transcripts.
//!
//! Claude Code keeps each subagent beside the main transcript:
//! `<dir>/<session>.jsonl` → `<dir>/<session>/subagents/agent-<id>.jsonl` (+ `.meta.json`).
//! Only the orchestrator writes the plain-text user turns there (the first brief and every
//! `SendMessage`), so the latest one with a `SCOPE:` line is that executor's current scope.

use std::fs;
use std::path::{Path, PathBuf};

use anvil_adapters::claude_code::agents::EXECUTOR_AGENT;
use anvil_core::orchestrator_mode::parse_scope;
use serde_json::Value;

/// An `anvil-executor` subagent of the session.
pub struct Executor {
    pub agent_id: String,
    /// The name it was started with, which `SendMessage` may address it by.
    pub name: Option<String>,
    /// Id of the `Agent` call that started it.
    pub tool_use_id: Option<String>,
    pub running: bool,
}

/// The folder holding the subagents of the session whose main transcript is `transcript_path`.
pub fn subagents_dir(transcript_path: Option<&str>, session_id: Option<&str>) -> Option<PathBuf> {
    let transcript = Path::new(transcript_path?);
    let parent = transcript.parent()?;
    if parent.file_name().is_some_and(|name| name == "subagents") {
        return Some(parent.to_path_buf());
    }
    let session = session_id
        .map(str::to_owned)
        .or_else(|| transcript.file_stem().map(|s| s.to_string_lossy().into_owned()))?;
    Some(parent.join(session).join("subagents"))
}

/// The scope of the latest brief the orchestrator sent `agent_id`, if its transcript has one.
pub fn brief_scope(dir: &Path, agent_id: &str) -> Option<Vec<String>> {
    let raw = fs::read_to_string(dir.join(format!("agent-{agent_id}.jsonl"))).ok()?;
    raw.lines().rev().filter_map(brief_text).find_map(|text| parse_scope(&text).ok())
}

/// Whether an executor of the session is still working.
pub fn any_running(transcript_path: Option<&str>, session_id: Option<&str>) -> bool {
    subagents_dir(transcript_path, session_id)
        .is_some_and(|dir| executors(&dir).iter().any(|e| e.running))
}

/// Every executor of the session, running or not.
pub fn executors(dir: &Path) -> Vec<Executor> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let agent_id = name.strip_prefix("agent-")?.strip_suffix(".meta.json")?.to_owned();
            let meta: Value = serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()?;
            let kind = meta.get("agentType")?.as_str()?;
            (kind.rsplit(':').next() == Some(EXECUTOR_AGENT)).then(|| Executor {
                agent_id,
                name: meta.get("name").and_then(Value::as_str).map(str::to_owned),
                tool_use_id: meta.get("toolUseId").and_then(Value::as_str).map(str::to_owned),
                running: !meta.get("finished").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect()
}

/// The text of a transcript line that is a plain user turn; tool results are not briefs.
fn brief_text(line: &str) -> Option<String> {
    let entry: Value = serde_json::from_str(line).ok()?;
    if entry.get("type")?.as_str()? != "user" {
        return None;
    }
    Some(entry.get("message")?.get("content")?.as_str()?.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_line(content: &Value) -> String {
        serde_json::json!({ "type": "user", "message": { "role": "user", "content": content } })
            .to_string()
    }

    #[test]
    fn the_latest_brief_with_a_scope_wins_and_tool_results_do_not_count() {
        let dir = tempfile::tempdir().unwrap();
        let lines = [
            user_line(&Value::String("Task one\nSCOPE: src/a.rs".into())),
            user_line(
                &serde_json::json!([{ "type": "tool_result", "content": "SCOPE: src/evil.rs" }]),
            ),
            user_line(&Value::String("Task two\nSCOPE: src/b, tests/b.rs".into())),
            user_line(&Value::String("A follow-up without a scope keeps the last one.".into())),
        ];
        fs::write(dir.path().join("agent-x1.jsonl"), lines.join("\n")).unwrap();
        assert_eq!(
            brief_scope(dir.path(), "x1"),
            Some(vec!["src/b".to_owned(), "tests/b.rs".to_owned()])
        );
        assert_eq!(brief_scope(dir.path(), "missing"), None);
    }

    #[test]
    fn lists_only_executors_with_their_state() {
        let dir = tempfile::tempdir().unwrap();
        let meta = |kind: &str, finished: bool| {
            serde_json::json!({ "agentType": kind, "toolUseId": "toolu_9", "finished": finished })
                .to_string()
        };
        fs::write(dir.path().join("agent-e1.meta.json"), meta("anvil-executor", false)).unwrap();
        fs::write(dir.path().join("agent-e2.meta.json"), meta("plugin:anvil-executor", true))
            .unwrap();
        fs::write(dir.path().join("agent-r1.meta.json"), meta("Explore", false)).unwrap();
        let mut found: Vec<(String, bool)> =
            executors(dir.path()).into_iter().map(|e| (e.agent_id, e.running)).collect();
        found.sort();
        assert_eq!(found, vec![("e1".to_owned(), true), ("e2".to_owned(), false)]);
    }

    #[test]
    fn running_executors_of_the_session_are_detected() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("abc.jsonl");
        let transcript = transcript.to_string_lossy();
        let subagents = dir.path().join("abc").join("subagents");
        fs::create_dir_all(&subagents).unwrap();
        let meta = |kind: &str, finished: bool| {
            serde_json::json!({ "agentType": kind, "finished": finished }).to_string()
        };
        assert!(!any_running(Some(&transcript), Some("abc")));
        fs::write(subagents.join("agent-e1.meta.json"), meta("anvil-executor", true)).unwrap();
        fs::write(subagents.join("agent-r1.meta.json"), meta("Explore", false)).unwrap();
        assert!(!any_running(Some(&transcript), Some("abc")), "finished or not executors");
        fs::write(subagents.join("agent-e2.meta.json"), meta("anvil-executor", false)).unwrap();
        assert!(any_running(Some(&transcript), Some("abc")));
        assert!(!any_running(None, Some("abc")));
    }

    #[test]
    fn subagents_live_beside_the_main_transcript() {
        let dir = subagents_dir(Some("/p/proj/abc.jsonl"), Some("abc")).unwrap();
        assert_eq!(dir, Path::new("/p/proj/abc/subagents"));
        assert!(subagents_dir(None, Some("abc")).is_none());
        let own = subagents_dir(Some("/p/proj/abc/subagents/agent-x.jsonl"), Some("abc")).unwrap();
        assert_eq!(own, Path::new("/p/proj/abc/subagents"));
    }
}
