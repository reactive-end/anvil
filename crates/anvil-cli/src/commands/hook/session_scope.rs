//! Session baselines limiting `Stop` gates to files changed since the agent session began.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anvil_core::fs_util::atomic_write;
use anvil_core::git::{self, DiffScope};
use serde::{Deserialize, Serialize};

const MAX_BASELINE_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Fingerprints (`len:mtime`) of files that were already dirty when the session began.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBaseline {
    dirty: BTreeMap<String, String>,
}

impl SessionBaseline {
    /// Keeps only the files whose content changed after the baseline was captured.
    pub fn changed_since(&self, root: &Path, changed: Vec<String>) -> Vec<String> {
        changed
            .into_iter()
            .filter(|rel| self.dirty.get(rel) != Some(&fingerprint(&root.join(rel))))
            .collect()
    }
}

/// Captures a baseline once per session. Hosts without session ids get one per fresh prompt.
pub fn ensure_baseline(root: &Path, session_id: Option<&str>, fresh_prompt: bool) {
    if !root.join(".harness").is_dir() {
        return;
    }
    let path = baseline_path(root, session_id);
    let reusable = session_id.is_some() || !fresh_prompt;
    if reusable && path.is_file() {
        return;
    }
    let Ok(changed) = git::changed_files(root, DiffScope::WorkingTree) else {
        return;
    };
    let dirty = changed
        .into_iter()
        .map(|rel| {
            let print = fingerprint(&root.join(&rel));
            (rel, print)
        })
        .collect();
    if let Ok(json) = serde_json::to_vec(&SessionBaseline { dirty }) {
        let _ = atomic_write(&path, &json);
        prune_stale(&sessions_dir(root));
    }
}

/// Loads the baseline of the running session, if one was captured.
pub fn load_baseline(root: &Path, session_id: Option<&str>) -> Option<SessionBaseline> {
    let raw = fs::read(baseline_path(root, session_id)).ok()?;
    serde_json::from_slice(&raw).ok()
}

fn sessions_dir(root: &Path) -> PathBuf {
    root.join(".harness").join(".state").join("sessions")
}

fn baseline_path(root: &Path, session_id: Option<&str>) -> PathBuf {
    let key: String = session_id
        .unwrap_or("default")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(64)
        .collect();
    let name = if key.is_empty() { "default" } else { &key };
    sessions_dir(root).join(format!("{name}.json"))
}

fn fingerprint(path: &Path) -> String {
    let Ok(meta) = fs::metadata(path) else {
        return "missing".to_owned();
    };
    let nanos = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    format!("{}:{nanos}", meta.len())
}

fn prune_stale(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|m| now.duration_since(m).is_ok_and(|age| age > MAX_BASELINE_AGE));
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}
