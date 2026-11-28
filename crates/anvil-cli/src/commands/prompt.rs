//! `anvil prompt`: help less technical users get good results. `improve` turns a rough request
//! into a structured prompt, `assist` sets how much the prompt hook adds on its own. The hook
//! itself lives in [`assist`], shared with the CLI through the same term, gather, and gap logic.

pub mod assist;
mod constraints;
mod gaps;
mod gather;
#[cfg(test)]
mod gather_tests;
mod grep_stage;
mod improve;
mod refs;
mod render;
mod session;
mod stem;
mod stopwords;
mod terms;
mod verify;

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_core::config::prompt_assist::PromptAssistMode;
use anyhow::Result;

use crate::cli::subcmds::PromptSubcmd;

/// Dispatches `anvil prompt <sub>`.
pub fn execute(root: &Path, sub: PromptSubcmd) -> Result<()> {
    match sub {
        PromptSubcmd::Improve { text, json } => improve::run(root, text, json),
        PromptSubcmd::Assist { mode } => assist_cmd(root, mode.as_deref()),
    }
}

fn assist_cmd(root: &Path, mode: Option<&str>) -> Result<()> {
    let mut config = AnvilConfig::load_or_default(root)?;
    if let Some(raw) = mode {
        config.prompt_assist.mode = PromptAssistMode::parse(raw)?;
        config.save(root)?;
    }
    let settings = config.prompt_assist;
    let verb = if mode.is_some() { "set to" } else { "is" };
    println!(
        "prompt assist {verb} {} (block cap {} tokens, skips prompts under {} words)",
        settings.mode.as_str(),
        settings.max_tokens,
        settings.min_words
    );
    Ok(())
}
