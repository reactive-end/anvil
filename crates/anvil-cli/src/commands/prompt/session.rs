//! Per-session memory of what the hook already said, so the same symbol is not repeated on every
//! prompt and the same tip is not shown twice. Stored in `.harness/.state/prompt-assist.json`.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anvil_core::fs_util::{atomic_write, with_file_lock};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::gaps::Gap;

/// An item injected within this many prompts of the current one is not injected again.
pub const REPEAT_WINDOW: u64 = 3;
/// Sessions kept in the file; the least recently used are dropped first.
const MAX_SESSIONS: usize = 16;
/// Items remembered per session.
const MAX_ITEMS: usize = 64;
const LOCK_TIMEOUT_MS: u64 = 1000;

fn state_path(root: &Path) -> std::path::PathBuf {
    root.join(".harness/.state/prompt-assist.json")
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    sessions: BTreeMap<String, SessionState>,
}

/// What one session has been told.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Prompts assisted so far in this session.
    prompts: u64,
    /// Fingerprint of the last prompt, so a hook that fires again for the same prompt (Antigravity
    /// runs `PreInvocation` before every model call) does not count it twice.
    last_prompt: u64,
    /// Unix seconds of the last update, for eviction.
    updated: u64,
    /// Item key -> number of the prompt it was injected with.
    items: BTreeMap<String, u64>,
    /// Gap keys whose tip was already shown.
    tips: Vec<String>,
}

impl SessionState {
    /// Counts a new assisted prompt and forgets items too old to matter. Returns `false` when
    /// `fingerprint` is the prompt already handled last, which is not a new prompt.
    pub fn begin_prompt(&mut self, fingerprint: u64) -> bool {
        if self.prompts > 0 && self.last_prompt == fingerprint {
            return false;
        }
        self.last_prompt = fingerprint;
        self.prompts += 1;
        let now = self.prompts;
        self.items.retain(|_, seen| now.saturating_sub(*seen) <= REPEAT_WINDOW);
        true
    }

    /// `true` when `key` was injected within the last [`REPEAT_WINDOW`] prompts.
    pub fn was_injected_recently(&self, key: &str) -> bool {
        self.items.get(key).is_some_and(|seen| self.prompts.saturating_sub(*seen) <= REPEAT_WINDOW)
    }

    /// Records that `key` was injected with the current prompt.
    pub fn remember(&mut self, key: &str) {
        self.items.insert(key.to_owned(), self.prompts);
        while self.items.len() > MAX_ITEMS {
            let oldest = self.items.iter().min_by_key(|(_, seen)| **seen).map(|(k, _)| k.clone());
            match oldest {
                Some(k) => self.items.remove(&k),
                None => break,
            };
        }
    }

    /// `true` when the tip for `gap` was already shown in this session.
    pub fn tip_shown(&self, gap: Gap) -> bool {
        self.tips.iter().any(|t| t == gap.key())
    }

    /// Records that the tip for `gap` was shown.
    pub fn mark_tip(&mut self, gap: Gap) {
        if !self.tip_shown(gap) {
            self.tips.push(gap.key().to_owned());
        }
    }
}

/// Runs `f` on the state of `session_id` under the file lock and saves the result. Anything that
/// goes wrong is added to `problems` and `None` is returned: the prompt is then assisted without
/// dedupe rather than not at all.
pub fn with_session<T>(
    root: &Path,
    session_id: Option<&str>,
    problems: &mut Vec<String>,
    f: impl FnOnce(&mut SessionState) -> T,
) -> Option<T> {
    let lock = root.join(".harness/.state/prompt-assist.lock");
    let key = session_key(session_id);
    let result = with_file_lock(&lock, LOCK_TIMEOUT_MS, || {
        let mut state = load(root, problems)?;
        let session = state.sessions.entry(key).or_default();
        let value = f(session);
        session.updated = unix_now();
        evict(&mut state);
        save(root, &state)?;
        Ok(value)
    });
    match result {
        Ok(value) => Some(value),
        Err(err) => {
            problems.push(format!("prompt-assist state: {err:#}"));
            None
        }
    }
}

/// FNV-1a hash of `text`: stable across runs and Rust versions, unlike the std hasher.
pub fn fingerprint(text: &str) -> u64 {
    text.bytes()
        .fold(0xcbf2_9ce4_8422_2325, |hash, b| (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

fn session_key(session_id: Option<&str>) -> String {
    let key: String = session_id
        .unwrap_or("default")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(64)
        .collect();
    if key.is_empty() { "default".to_owned() } else { key }
}

/// Reads the state; a corrupt file is reported and replaced by an empty one.
fn load(root: &Path, problems: &mut Vec<String>) -> Result<State> {
    let path = state_path(root);
    if !path.is_file() {
        return Ok(State::default());
    }
    let raw = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    match serde_json::from_slice(&raw) {
        Ok(state) => Ok(state),
        Err(err) => {
            problems.push(format!("prompt-assist state was corrupt and was reset: {err}"));
            Ok(State::default())
        }
    }
}

fn save(root: &Path, state: &State) -> Result<()> {
    let json = serde_json::to_vec(state).context("failed to serialize prompt-assist state")?;
    atomic_write(&state_path(root), &json)
}

fn evict(state: &mut State) {
    while state.sessions.len() > MAX_SESSIONS {
        let oldest = state.sessions.iter().min_by_key(|(_, s)| s.updated).map(|(k, _)| k.clone());
        match oldest {
            Some(k) => state.sessions.remove(&k),
            None => break,
        };
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_is_not_repeated_for_three_prompts_then_may_return() {
        let mut s = SessionState::default();
        s.begin_prompt(0);
        s.remember("code:a.ts:Order");
        for n in 1..=REPEAT_WINDOW {
            s.begin_prompt(n);
            assert!(s.was_injected_recently("code:a.ts:Order"));
        }
        s.begin_prompt(99);
        assert!(!s.was_injected_recently("code:a.ts:Order"));
    }

    #[test]
    fn the_same_prompt_twice_in_a_row_counts_once() {
        let mut s = SessionState::default();
        assert!(s.begin_prompt(fingerprint("fix the login")));
        assert!(!s.begin_prompt(fingerprint("fix the login")));
        assert!(s.begin_prompt(fingerprint("now the cart")));
    }

    #[test]
    fn a_tip_is_shown_once_per_gap_kind() {
        let mut s = SessionState::default();
        assert!(!s.tip_shown(Gap::NoTarget));
        s.mark_tip(Gap::NoTarget);
        s.mark_tip(Gap::NoTarget);
        assert!(s.tip_shown(Gap::NoTarget));
        assert!(!s.tip_shown(Gap::BroadScope));
        assert_eq!(s.tips.len(), 1);
    }

    #[test]
    fn remembered_items_stay_bounded() {
        let mut s = SessionState::default();
        for n in 0..(MAX_ITEMS + 20) {
            s.begin_prompt(n as u64);
            s.remember(&format!("k{n}"));
        }
        assert!(s.items.len() <= MAX_ITEMS);
    }

    #[test]
    fn state_persists_per_session_and_stays_bounded() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join(".harness/.state")).unwrap();
        let mut problems = Vec::new();
        for n in 0..(MAX_SESSIONS + 5) {
            let id = format!("s{n}");
            with_session(dir.path(), Some(&id), &mut problems, |s| {
                s.begin_prompt(1);
                s.remember("x");
            });
        }
        let seen = with_session(dir.path(), Some("s20"), &mut problems, |s| {
            s.begin_prompt(2);
            s.was_injected_recently("x")
        });
        assert_eq!(seen, Some(true));
        let raw = fs::read(state_path(dir.path())).unwrap();
        let state: State = serde_json::from_slice(&raw).unwrap();
        assert!(state.sessions.len() <= MAX_SESSIONS);
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn a_corrupt_file_is_reported_and_replaced() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join(".harness/.state")).unwrap();
        fs::write(state_path(dir.path()), "not json").unwrap();
        let mut problems = Vec::new();
        let ran = with_session(dir.path(), None, &mut problems, |s| s.begin_prompt(1));
        assert!(ran.is_some());
        assert!(problems.iter().any(|p| p.contains("corrupt")), "{problems:?}");
        assert!(
            serde_json::from_slice::<State>(&fs::read(state_path(dir.path())).unwrap()).is_ok()
        );
    }
}
