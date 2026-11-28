//! The `UserPromptSubmit` / `PreInvocation` addition: a small context block for the agent and, in
//! coach mode, a clarify-first nudge for the agent plus a short tip for the user. Deterministic:
//! no model call, no network, and a hard time budget so the hook stays fast.

use std::path::Path;
use std::time::Duration;

use anvil_adapters::contract::{InboxMessage, MessageLevel, NormalizedHookInput};
use anvil_core::config::AnvilConfig;
use anvil_core::config::prompt_assist::{PromptAssistConfig, PromptAssistMode};
use anvil_core::user_config::UserConfig;

use super::gaps::{self, Gap};
use super::gather::{self, Limits};
use super::refs::Gathered;
use super::render::{self, Fitted};
use super::session::{self, SessionState};
use super::terms;
use super::verify::QaTargets;
use crate::commands::hook::error_log;

/// Everything gathering may take before the remaining stages are skipped.
pub const TIME_BUDGET: Duration = Duration::from_millis(250);
const LIMITS: Limits = Limits { code: 6, docs: 3, memories: 3, time: TIME_BUDGET, grep: false };

/// What the hook adds for one prompt.
#[derive(Debug, Default)]
pub struct Assist {
    /// For the agent: `prompt-context` and `prompt-coach`.
    pub messages: Vec<InboxMessage>,
    /// For the user, one line, when a gap has not been mentioned yet in this session.
    pub user_tip: Option<String>,
}

/// Builds the assistance for `input`, or nothing when the mode is off or the prompt is skipped.
/// The mode is the user's global setting; a missing file means off, an unreadable one is logged
/// and also means off.
pub fn on_prompt(root: &Path, config: &AnvilConfig, input: &NormalizedHookInput) -> Assist {
    if input.user_prompt.is_none() {
        return Assist::default();
    }
    let settings = match UserConfig::load() {
        Ok(user) => user.prompt_assist,
        Err(err) => {
            error_log::log_hook_error(root, "prompt-assist", &format!("{err:#}"));
            return Assist::default();
        }
    };
    assist_with(root, config, input, settings)
}

fn assist_with(
    root: &Path,
    config: &AnvilConfig,
    input: &NormalizedHookInput,
    settings: PromptAssistConfig,
) -> Assist {
    let Some(prompt) = input.user_prompt.as_deref() else {
        return Assist::default();
    };
    if settings.mode == PromptAssistMode::Off
        || !root.join(".harness").is_dir()
        || terms::skip_reason(prompt, settings.min_words).is_some()
    {
        return Assist::default();
    }
    let found = terms::extract(prompt);
    if found.is_empty() {
        return Assist::default();
    }
    let mut problems = Vec::new();
    let mut gathered = gather::gather(root, config, &found, LIMITS);
    problems.append(&mut gathered.problems);
    let assist = assemble(root, config, input, prompt, gathered, settings, &mut problems);
    for problem in problems {
        error_log::log_hook_error(root, "prompt-assist", &problem);
    }
    assist
}

fn assemble(
    root: &Path,
    config: &AnvilConfig,
    input: &NormalizedHookInput,
    prompt: &str,
    gathered: Gathered,
    settings: PromptAssistConfig,
    problems: &mut Vec<String>,
) -> Assist {
    let coaching = settings.mode == PromptAssistMode::Coach;
    let all_gaps = if coaching { gaps::detect(prompt, &gathered) } else { Vec::new() };
    let gaps = if gaps::worth_coaching(&all_gaps) { all_gaps } else { Vec::new() };
    let commands = if gaps.is_empty() {
        Vec::new()
    } else {
        QaTargets::load(root, config, problems).commands()
    };
    let cap = render::cap_chars(settings.max_tokens);
    let fingerprint = session::fingerprint(prompt);
    let stateful = session::with_session(root, input.session_id.as_deref(), problems, |state| {
        if !state.begin_prompt(fingerprint) {
            return None;
        }
        let fresh = without_recent(&gathered, state);
        let fitted = render::fit(&fresh, &gaps, &commands, cap);
        remember_rendered(state, &fresh, &fitted);
        Some((fitted, next_tip(state, &gaps)))
    });
    match stateful {
        Some(Some((fitted, tip))) => to_assist(fitted, tip),
        // Same prompt seen again (a later model call): already assisted, add nothing.
        Some(None) => Assist::default(),
        // No state could be kept: still help, but skip the tip, which could not be limited.
        None => to_assist(render::fit(&gathered, &gaps, &commands, cap), None),
    }
}

/// `gathered` without the items already injected within the last prompts of this session.
fn without_recent(gathered: &Gathered, state: &SessionState) -> Gathered {
    Gathered {
        code: gathered
            .code
            .iter()
            .filter(|c| !state.was_injected_recently(&c.key()))
            .cloned()
            .collect(),
        docs: gathered
            .docs
            .iter()
            .filter(|d| !state.was_injected_recently(&d.key()))
            .cloned()
            .collect(),
        memories: gathered
            .memories
            .iter()
            .filter(|m| !state.was_injected_recently(&m.key()))
            .cloned()
            .collect(),
        ..gathered.clone()
    }
}

/// Remembers the items that made it into the rendered line (not the ones cut for space).
fn remember_rendered(state: &mut SessionState, fresh: &Gathered, fitted: &Fitted) {
    let Some(line) = fitted.context.as_deref() else {
        return;
    };
    for code in fresh.code.iter().filter(|c| line.contains(&c.render())) {
        state.remember(&code.key());
    }
    for doc in fresh.docs.iter().filter(|d| line.contains(&d.render())) {
        state.remember(&doc.key());
    }
    for memory in fresh.memories.iter().filter(|m| line.contains(&m.render())) {
        state.remember(&memory.key());
    }
}

/// The first gap whose tip this session has not shown yet; marks it shown.
fn next_tip(state: &mut SessionState, gaps: &[Gap]) -> Option<String> {
    let gap = gaps.iter().copied().find(|g| !state.tip_shown(*g))?;
    state.mark_tip(gap);
    Some(render::user_tip(gap))
}

fn to_assist(fitted: Fitted, user_tip: Option<String>) -> Assist {
    let mut messages = Vec::new();
    for (source, text) in [("prompt-context", fitted.context), ("prompt-coach", fitted.coach)] {
        if let Some(text) = text {
            messages.push(InboxMessage::new(source, MessageLevel::Info, text, None));
        }
    }
    Assist { messages, user_tip }
}
