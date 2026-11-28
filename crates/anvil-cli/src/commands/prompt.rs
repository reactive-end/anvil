//! `anvil prompt`: help less technical users get good results. `improve` turns a rough request
//! into a structured prompt, `assist` sets, globally for the user, how much the prompt hook adds
//! on its own (off by default). The hook
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

use anvil_core::config::prompt_assist::PromptAssistMode;
use anvil_core::user_config::{self, UserConfig};
use anyhow::Result;

use crate::cli::subcmds::PromptSubcmd;

/// Dispatches `anvil prompt <sub>`.
pub fn execute(root: &Path, sub: PromptSubcmd) -> Result<()> {
    match sub {
        PromptSubcmd::Improve { text, json } => improve::run(root, text, json),
        PromptSubcmd::Assist { mode, max_tokens, min_words } => {
            assist_cmd(mode.as_deref(), max_tokens, min_words)
        }
    }
}

/// Shows or changes the user's global prompt assist settings (`~/.anvil/config.json`).
fn assist_cmd(mode: Option<&str>, max_tokens: Option<u32>, min_words: Option<u32>) -> Result<()> {
    let path = user_config::config_path()?;
    let mut config = UserConfig::load_from(&path)?;
    let changing = mode.is_some() || max_tokens.is_some() || min_words.is_some();
    if let Some(raw) = mode {
        config.prompt_assist.mode = PromptAssistMode::parse(raw)?;
    }
    if let Some(tokens) = max_tokens {
        config.prompt_assist.max_tokens = usize::try_from(tokens)?;
    }
    if let Some(words) = min_words {
        config.prompt_assist.min_words = usize::try_from(words)?;
    }
    if changing {
        config.save_to(&path)?;
    }
    let settings = config.prompt_assist;
    let verb = if changing { "set to" } else { "is" };
    println!("prompt assist {verb} {} (global: {})", settings.mode.as_str(), path.display());
    println!(
        "block cap {} tokens, skips prompts under {} words",
        settings.max_tokens, settings.min_words
    );
    Ok(())
}
