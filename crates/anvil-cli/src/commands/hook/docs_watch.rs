//! Reactive documentation hooks: remind about concepts when their code is edited, validate edited
//! concepts, and advise at `Stop` about concepts that were not refreshed.
//!
//! Only frontmatter is read (never the bodies), nothing is cached across hook calls, and the hooks do
//! nothing while `.harness/docs/` holds no bundle.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anvil_adapters::contract::{InboxMessage, MessageLevel};
use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::atomic_write;
use anvil_docs::bundle;
use anvil_docs::concept::{Concept, scan_all};
use anvil_docs::drift::documenting;
use anvil_docs::repo_files;
use anvil_docs::validate::{Severity, validate_file};
use serde::{Deserialize, Serialize};

use super::error_log::log_hook_error;

const TURN_STATE: &str = ".harness/.state/docs-turn.json";
const SOURCE: &str = "anvil-docs";

/// Concepts already reminded about this turn, with the time of the first reminder.
#[derive(Debug, Default, Serialize, Deserialize)]
struct TurnState {
    concepts: BTreeMap<String, String>,
}

fn load_state(root: &Path) -> TurnState {
    fs::read_to_string(root.join(TURN_STATE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_state(root: &Path, state: &TurnState) {
    let Ok(json) = serde_json::to_string(state) else {
        return;
    };
    if let Err(err) = atomic_write(&root.join(TURN_STATE), json.as_bytes()) {
        log_hook_error(root, "post-tool-use:docs-state", &err);
    }
}

/// Forgets which concepts were reminded about; called when a new turn starts or a `Stop` finishes.
pub fn clear_turn(root: &Path) {
    let _ = fs::remove_file(root.join(TURN_STATE));
}

/// Messages for an edit of the repo file `rel`: a reminder for each concept documenting it (once per
/// turn), or the validation errors when `rel` is itself a document.
pub fn on_edit(root: &Path, config: &AnvilConfig, rel: &str) -> Vec<InboxMessage> {
    if !bundle::has_bundles(root) {
        return Vec::new();
    }
    if rel.starts_with(".harness/docs/") {
        return validation_message(root, config, rel).into_iter().collect();
    }
    if !config.docs.remind_on_edit {
        return Vec::new();
    }
    reminders(root, config, rel)
}

fn reminders(root: &Path, config: &AnvilConfig, rel: &str) -> Vec<InboxMessage> {
    let bundles = bundle::existing(root, config);
    let concepts = scan_all(root, &bundles, false);
    let hits = documenting(&concepts, rel);
    if hits.is_empty() {
        return Vec::new();
    }
    let mut state = load_state(root);
    let mut messages = Vec::new();
    for concept in hits {
        let key = concept.key();
        if state.concepts.contains_key(&key) {
            continue;
        }
        state.concepts.insert(key.clone(), anvil_docs::now_rfc3339());
        messages.push(InboxMessage::new(
            SOURCE,
            MessageLevel::Info,
            format!(
                "`{}` documents this file: update it if behaviour changed, then run `anvil docs refresh {key}`.",
                concept.rel_path
            ),
            None,
        ));
    }
    if !messages.is_empty() {
        save_state(root, &state);
    }
    messages
}

fn validation_message(root: &Path, config: &AnvilConfig, rel: &str) -> Option<InboxMessage> {
    let project = bundle::project_of_doc_path(rel)?;
    if !rel.ends_with(".md") {
        return None;
    }
    let found = bundle::existing(root, config).into_iter().find(|b| b.project == project)?;
    let files = repo_files::list(root);
    let problems: Vec<String> = validate_file(root, &found, &root.join(rel), &files)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .map(|issue| issue.message)
        .collect();
    if problems.is_empty() {
        return None;
    }
    let text = format!(
        "`{rel}` is not valid OKF: {}. Fix it, then run `anvil docs check`.",
        problems.join("; ")
    );
    Some(InboxMessage::new(SOURCE, MessageLevel::Warning, text, None))
}

/// Non-blocking advisory at `Stop`: concepts whose code changed this turn but that were not refreshed.
pub fn stop_advisory(root: &Path, config: &AnvilConfig) -> Option<InboxMessage> {
    let state = load_state(root);
    if state.concepts.is_empty() {
        return None;
    }
    let bundles = bundle::existing(root, config);
    let concepts = scan_all(root, &bundles, false);
    let pending: Vec<String> = state
        .concepts
        .iter()
        .filter(|(key, since)| {
            concepts.iter().any(|c| c.key() == **key && !refreshed_since(c, since))
        })
        .map(|(key, _)| key.clone())
        .collect();
    clear_turn(root);
    if pending.is_empty() {
        return None;
    }
    let text = format!(
        "Code documented by {} changed this session but the concept was not refreshed. If behaviour changed, update the doc and run `anvil docs refresh <id>`.",
        pending.join(", ")
    );
    Some(InboxMessage::new(SOURCE, MessageLevel::Warning, text, None))
}

fn refreshed_since(concept: &Concept, since: &str) -> bool {
    let Ok(since) = chrono::DateTime::parse_from_rfc3339(since) else {
        return false;
    };
    concept.doc.generated().and_then(|g| g.instant()).is_some_and(|at| at > since)
}
