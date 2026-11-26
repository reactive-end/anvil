//! Review gate for orchestrator mode: a task with changed files closes only after the orchestrator
//! read a review packet of its current content and approved it.
//!
//! The packet and the approval are bound to a fingerprint of the task's files, so any edit after
//! the review invalidates it and the change has to be reviewed again.

mod packet;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::fs_util::atomic_write;
use crate::orchestrator_mode;
use crate::stats::scope_key;

pub use packet::build_packet;

/// A review packet the orchestrator was shown for a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketRecord {
    pub fingerprint: String,
    pub files: Vec<String>,
    pub at: DateTime<Utc>,
}

/// The orchestrator's approval of a task's reviewed content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub fingerprint: String,
    pub summary: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct GateState {
    #[serde(default)]
    packets: BTreeMap<String, PacketRecord>,
    #[serde(default)]
    approvals: BTreeMap<String, Approval>,
}

/// The fingerprint of `files` as they are now; deleted files count as deleted.
pub fn fingerprint(repo_root: &Path, files: &[String]) -> String {
    scope_key(repo_root, files)
}

/// Remembers that a packet of `files` was shown for `task_id`, replacing any earlier one.
pub fn record_packet(repo_root: &Path, task_id: &str, files: &[String]) -> Result<PacketRecord> {
    let record = PacketRecord {
        fingerprint: fingerprint(repo_root, files),
        files: files.to_vec(),
        at: Utc::now(),
    };
    let mut state = load(repo_root)?;
    state.packets.insert(task_id.to_owned(), record.clone());
    save(repo_root, &state)?;
    Ok(record)
}

/// Approves `task_id`. Refuses when no packet was shown for it, or its files changed since.
pub fn approve(repo_root: &Path, task_id: &str, summary: &str) -> Result<Approval> {
    let summary = summary.trim();
    if summary.is_empty() {
        bail!("say what you verified, for example \"checked error paths and the new tests\"");
    }
    let mut state = load(repo_root)?;
    let Some(packet) = state.packets.get(task_id) else {
        bail!(
            "no review packet for {task_id}: read `anvil review packet {task_id}` (MCP `review_packet`) first"
        );
    };
    let current = fingerprint(repo_root, &packet.files);
    if current != packet.fingerprint {
        bail!(
            "the files of {task_id} changed after its review packet: build and read a new packet before approving"
        );
    }
    let approval = Approval { fingerprint: current, summary: summary.to_owned(), at: Utc::now() };
    state.approvals.insert(task_id.to_owned(), approval.clone());
    save(repo_root, &state)?;
    Ok(approval)
}

/// In orchestrator mode, refuses to close a task whose current `files` were not approved.
///
/// Outside the mode, and for tasks without changed files, there is nothing to enforce.
pub fn ensure_reviewed(repo_root: &Path, task_id: &str, files: &[String]) -> Result<()> {
    if files.is_empty() || !orchestrator_mode::is_enabled(repo_root) {
        return Ok(());
    }
    let state = load(repo_root)?;
    let current = fingerprint(repo_root, files);
    match state.approvals.get(task_id) {
        Some(approval) if approval.fingerprint == current => Ok(()),
        Some(_) => bail!(
            "{task_id} changed after its review was approved: build a new packet with `anvil review packet {task_id}`, review it, and approve again"
        ),
        None => bail!(
            "orchestrator mode: review {task_id} before closing it. Read `anvil review packet {task_id}` (MCP `review_packet`), send problems back to the executor and log them with `review_log`, then run `anvil review approve {task_id} \"<what you verified>\"` (MCP `review_approve`)"
        ),
    }
}

fn state_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".harness").join(".state").join("review_gate.json")
}

fn load(repo_root: &Path) -> Result<GateState> {
    let path = state_path(repo_root);
    match fs::read_to_string(&path) {
        Ok(raw) if !raw.trim().is_empty() => serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", path.display())),
        _ => Ok(GateState::default()),
    }
}

fn save(repo_root: &Path, state: &GateState) -> Result<()> {
    let path = state_path(repo_root);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(state).context("failed to serialize review gate")?;
    atomic_write(&path, json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tmp");
        fs::create_dir_all(tmp.path().join(".harness/.state")).expect("state dir");
        fs::write(tmp.path().join("a.rs"), "fn a() {}\n").expect("write");
        tmp
    }

    fn files() -> Vec<String> {
        vec!["a.rs".to_owned()]
    }

    #[test]
    fn approval_needs_a_current_packet_and_a_summary() {
        let tmp = repo();
        let root = tmp.path();
        assert!(approve(root, "T-1", "checked").is_err(), "no packet yet");
        record_packet(root, "T-1", &files()).expect("packet");
        assert!(approve(root, "T-1", "  ").is_err(), "empty summary");
        fs::write(root.join("a.rs"), "fn a() { b() }\n").expect("edit");
        assert!(approve(root, "T-1", "checked").is_err(), "edited after the packet");
        record_packet(root, "T-1", &files()).expect("packet");
        assert_eq!(approve(root, "T-1", "checked").expect("approve").summary, "checked");
    }

    #[test]
    fn closing_requires_approval_of_the_current_content_only_in_orchestrator_mode() {
        let tmp = repo();
        let root = tmp.path();
        ensure_reviewed(root, "T-1", &files()).expect("mode off: nothing to enforce");
        orchestrator_mode::set_enabled(root, true).expect("mode on");
        ensure_reviewed(root, "T-1", &[]).expect("no files: nothing to review");
        assert!(ensure_reviewed(root, "T-1", &files()).is_err(), "not reviewed");
        record_packet(root, "T-1", &files()).expect("packet");
        approve(root, "T-1", "checked").expect("approve");
        ensure_reviewed(root, "T-1", &files()).expect("approved");
        fs::write(root.join("a.rs"), "fn a() { c() }\n").expect("edit");
        let err = ensure_reviewed(root, "T-1", &files()).expect_err("changed after approval");
        assert!(err.to_string().contains("changed after its review"), "{err}");
    }
}
