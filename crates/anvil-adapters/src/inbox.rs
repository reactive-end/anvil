//! Persistent `.harness/.state/inbox.json` queue bridging Antigravity's
//! `PostToolUse -> PreInvocation` feedback asymmetry.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use anvil_core::fs_util::{atomic_write, with_file_lock};
use serde::{Deserialize, Serialize};

use crate::contract::{InboxMessage, MessageLevel};

const INBOX_LOCK_TIMEOUT_MS: u64 = 2000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InboxStore {
    version: u32,
    messages: Vec<InboxMessage>,
}

impl Default for InboxStore {
    fn default() -> Self {
        Self {
            version: 1,
            messages: Vec::new(),
        }
    }
}

/// Returns the canonical path to `.harness/.state/inbox.json`.
pub fn inbox_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".harness").join(".state").join("inbox.json")
}

/// Returns the advisory lock path `.harness/.state/inbox.lock`.
pub fn inbox_lock_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".harness").join(".state").join("inbox.lock")
}

/// Loads pending inbox messages without modifying the queue.
pub fn load_inbox(repo_root: &Path) -> Result<Vec<InboxMessage>> {
    load_inbox_unlocked(repo_root)
}

/// Appends `messages` to `.harness/.state/inbox.json` atomically under advisory file lock.
pub fn push_messages(repo_root: &Path, messages: &[InboxMessage]) -> Result<()> {
    if messages.is_empty() {
        return Ok(());
    }
    with_file_lock(&inbox_lock_path(repo_root), INBOX_LOCK_TIMEOUT_MS, || {
        let mut existing = load_inbox_unlocked(repo_root)?;
        existing.extend(messages.iter().cloned());
        save_inbox_unlocked(repo_root, existing)
    })
}

/// Atomically drains and clears all pending messages from `.harness/.state/inbox.json`.
pub fn drain_inbox(repo_root: &Path) -> Result<Vec<InboxMessage>> {
    with_file_lock(&inbox_lock_path(repo_root), INBOX_LOCK_TIMEOUT_MS, || {
        let messages = load_inbox_unlocked(repo_root)?;
        if !messages.is_empty() {
            clear_inbox_unlocked(repo_root)?;
        }
        Ok(messages)
    })
}

/// Clears `.harness/.state/inbox.json` if it exists under advisory file lock.
pub fn clear_inbox(repo_root: &Path) -> Result<()> {
    with_file_lock(&inbox_lock_path(repo_root), INBOX_LOCK_TIMEOUT_MS, || {
        clear_inbox_unlocked(repo_root)
    })
}

/// Formats queued or inline messages into a compact system/context string.
pub fn format_context(messages: &[InboxMessage]) -> Option<String> {
    if messages.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(messages.len() * 96);
    for (idx, msg) in messages.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        let lvl = match msg.level {
            MessageLevel::Info => "info",
            MessageLevel::Warning => "warn",
            MessageLevel::Error => "error",
        };
        let _ = write!(out, "[{lvl}][{}]", msg.source);
        if let Some(file) = &msg.file {
            let _ = write!(out, " ({file})");
        }
        let _ = write!(out, ": {}", msg.content);
    }
    Some(out)
}

pub(crate) fn format_stop_reason(
    reason: Option<&str>,
    messages: &[InboxMessage],
) -> Option<String> {
    let formatted = format_context(messages);
    match (reason, formatted) {
        (Some(r), Some(m)) if !r.is_empty() => Some(format!("{r}\n{m}")),
        (Some(_), Some(m)) => Some(m),
        (Some(r), None) => Some(r.to_owned()),
        (None, Some(m)) => Some(m),
        (None, None) => None,
    }
}

fn load_inbox_unlocked(repo_root: &Path) -> Result<Vec<InboxMessage>> {
    let path = inbox_path(repo_root);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read inbox at {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let store: InboxStore = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse inbox JSON at {}", path.display()))?;
    Ok(store.messages)
}

fn clear_inbox_unlocked(repo_root: &Path) -> Result<()> {
    let path = inbox_path(repo_root);
    if !path.exists() {
        return Ok(());
    }
    save_inbox_unlocked(repo_root, Vec::new())
}

fn save_inbox_unlocked(repo_root: &Path, messages: Vec<InboxMessage>) -> Result<()> {
    let path = inbox_path(repo_root);
    let store = InboxStore {
        version: 1,
        messages,
    };
    let json = serde_json::to_string(&store).context("failed to serialize inbox")?;
    atomic_write(&path, json.as_bytes())
}
