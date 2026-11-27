//! Per-turn, per-agent record of who already consulted the graph and docs.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anvil_adapters::contract::NormalizedHookInput;
use anvil_core::fs_util::{atomic_write, with_file_lock};
use serde::{Deserialize, Serialize};

use super::log_hook_error;

const STATE_FILE: &str = ".harness/.state/search-cascade.json";
const LOCK_FILE: &str = ".harness/.state/search-cascade.lock";
const LOCK_TIMEOUT_MS: u64 = 500;

/// Keys (`session:agent`) of the agents that consulted the graph or docs this turn.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Consulted {
    agents: BTreeSet<String>,
}

/// `<session>:<agent_id>` for a subagent, `<session>:main` for the main thread.
fn agent_key(input: &NormalizedHookInput) -> String {
    let session = input.session_id.as_deref().unwrap_or_default();
    let agent = input.agent_id.as_deref().filter(|id| !id.is_empty()).unwrap_or("main");
    format!("{session}:{agent}")
}

fn load(root: &Path) -> Consulted {
    fs::read_to_string(root.join(STATE_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Whether the agent behind `input` already looked in the graph or docs this turn.
pub fn is_consulted(root: &Path, input: &NormalizedHookInput) -> bool {
    load(root).agents.contains(&agent_key(input))
}

/// Records that the agent behind `input` consulted the graph or docs; a no-op when already recorded.
pub fn mark_consulted(root: &Path, input: &NormalizedHookInput) {
    if is_consulted(root, input) {
        return;
    }
    let key = agent_key(input);
    let result = with_file_lock(&root.join(LOCK_FILE), LOCK_TIMEOUT_MS, || {
        let mut state = load(root);
        state.agents.insert(key);
        atomic_write(&root.join(STATE_FILE), serde_json::to_string(&state)?.as_bytes())
    });
    if let Err(err) = result {
        log_hook_error(root, "pre-tool-use:search-state", &err);
    }
}

/// Forgets every consultation; called when a new user prompt starts a turn.
pub fn clear_turn(root: &Path) {
    let _ = fs::remove_file(root.join(STATE_FILE));
}
