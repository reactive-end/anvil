//! Enforces the lookup order code graph -> docs -> code: the first code search of a turn (per agent)
//! is answered with the graph and docs results for the pattern; repeating it goes through.
//!
//! A turn counts as consulted once the agent used `search`, a `graph_*` tool, `context_pack`,
//! `code_exec`, read a concept under `.harness/docs/`, or ran `anvil search|graph|docs search|context`.
//! The flags live in `.harness/.state/search-cascade.json` and are cleared on `UserPromptSubmit`.

mod detect;
mod state;
#[cfg(test)]
mod tests;

use std::path::Path;

use anvil_adapters::contract::{HookOutcome, InboxMessage, MessageLevel, NormalizedHookInput};
use anvil_adapters::inbox;
use anvil_core::config::AnvilConfig;
use anvil_core::config::search::SearchCascadeMode;
use anvil_docs::bundle;
use anvil_graph::store;
use anvil_mcp::search_cascade::{CascadeOptions, CascadeReport, CodeStage, cascade, render};

use super::error_log::log_hook_error;
use detect::{Call, SearchCall};

pub use state::clear_turn;

/// Token cap of the results quoted back in a denial.
const GUARD_TOKENS: usize = 600;
const SOURCE: &str = "anvil-search";
const REPEAT_HINT: &str =
    "Graph and docs were checked first; repeat the search if you still need the code.";

/// Answers the first code search of the turn with graph and docs results (`enforce`), or queues an
/// advisory (`advise`). Returns `None` to let the call through.
pub fn check_search_guard(
    root: &Path,
    config: &AnvilConfig,
    input: &NormalizedHookInput,
) -> Option<HookOutcome> {
    let mode = config.search.cascade;
    if mode == SearchCascadeMode::Off || !root.join(".harness").is_dir() {
        return None;
    }
    match detect::classify(root, input) {
        Call::Other => None,
        Call::Consult => {
            state::mark_consulted(root, input);
            None
        }
        Call::Search(call) => guard_search(root, config, input, mode, &call),
    }
}

fn guard_search(
    root: &Path,
    config: &AnvilConfig,
    input: &NormalizedHookInput,
    mode: SearchCascadeMode,
    call: &SearchCall,
) -> Option<HookOutcome> {
    if state::is_consulted(root, input) {
        return None;
    }
    if !store::default_bin_path(root).is_file() && !bundle::has_bundles(root) {
        return None;
    }
    let opts =
        CascadeOptions { code: CodeStage::Skip, max_tokens: GUARD_TOKENS, ..Default::default() };
    let report = cascade(root, config, &call.query, &opts);
    state::mark_consulted(root, input);
    if report.found() == 0 {
        return None;
    }
    if mode == SearchCascadeMode::Advise {
        advise(root, &report);
        return None;
    }
    Some(HookOutcome::deny(denial(&report)))
}

fn denial(report: &CascadeReport) -> String {
    format!(
        "Search cascade (graph -> docs -> code): the graph and docs were looked up before the code.\n{}\n{REPEAT_HINT}",
        render(report)
    )
}

/// Queues a one-line advisory; `PreToolUse` output cannot carry context, so it is delivered with the
/// next `PostToolUse`.
fn advise(root: &Path, report: &CascadeReport) {
    let message = InboxMessage::new(SOURCE, MessageLevel::Info, advisory(report), None);
    if let Err(err) = inbox::push_messages(root, &[message]) {
        log_hook_error(root, "pre-tool-use:search-advisory", &err);
    }
}

fn advisory(report: &CascadeReport) -> String {
    let top = match (report.graph.hits.first(), report.docs.hits.first()) {
        (Some(hit), _) => format!("graph: {} at {}:{}", hit.label, hit.file, hit.line),
        (None, Some(doc)) => format!("docs: {}", doc.path),
        (None, None) => String::new(),
    };
    format!(
        "Search cascade: {} graph and {} docs hit(s) for \"{}\" ({top}); try `search` or `graph_read_symbol` before grep.",
        report.graph.hits.len(),
        report.docs.hits.len(),
        report.query
    )
}
