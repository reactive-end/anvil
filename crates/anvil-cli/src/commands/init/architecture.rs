//! `anvil init` step that records the architecture style of each project.
//!
//! `--architecture <style>` is applied as given. With `auto`, the style is detected from the folder
//! names and, when one is found, the user picks it interactively (default: the detected one); without a
//! prompt it is only suggested, because enforcing a guessed style could fail builds unexpectedly.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use anvil_adapters::sync;
use anvil_core::config::AnvilConfig;
use anvil_core::config::architecture::detect::{Suggestion, detect_style};
use anvil_core::config::architecture::{ArchitectureStyle, DEFAULT_SOURCE_ROOT};
use anyhow::Result;

use crate::commands::ExitCodeError;
use crate::ui::prompt::{self, PromptOutcome, SelectOption};

/// The `--architecture` flag.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ArchMode {
    /// Detect, then prompt (interactive) or only suggest.
    Auto,
    /// Apply this style to every project.
    Explicit(ArchitectureStyle),
}

impl ArchMode {
    pub(super) fn parse(raw: &str) -> Result<Self> {
        if raw.trim().eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        Ok(Self::Explicit(raw.parse()?))
    }
}

/// A project whose style init may record: the repository root or one subrepo.
pub(super) struct Candidate {
    label: String,
    subrepo: Option<String>,
    source_dir: PathBuf,
    existing: ArchitectureStyle,
}

impl Candidate {
    /// The project in `dir` itself (its own `.harness/config.json`).
    pub(super) fn root(label: &str, dir: &Path) -> Result<Self> {
        let existing = AnvilConfig::load_or_default(dir)?.architecture.repo.style;
        Ok(Self {
            label: label.to_owned(),
            subrepo: None,
            source_dir: dir.join(DEFAULT_SOURCE_ROOT),
            existing,
        })
    }

    /// A subrepo `name` at `dir`, recorded in the workspace root config `root_config`.
    pub(super) fn subrepo(label: &str, name: &str, dir: &Path, root_config: &AnvilConfig) -> Self {
        let existing = root_config.architecture.subrepos.get(name).map(|s| s.style);
        Self {
            label: label.to_owned(),
            subrepo: Some(name.to_owned()),
            source_dir: dir.join(DEFAULT_SOURCE_ROOT),
            existing: existing.unwrap_or_default(),
        }
    }
}

/// What init decided to record, plus suggestions it could not apply without asking.
#[derive(Default)]
pub(super) struct ArchPlan {
    root: Option<ArchitectureStyle>,
    subrepos: Vec<(String, ArchitectureStyle)>,
    hints: Vec<String>,
}

/// How prompts reach the user.
pub(super) struct PromptIo<'a> {
    pub interactive: bool,
    pub use_tui: bool,
    pub reader: &'a mut dyn BufRead,
}

/// Decides the style of every candidate.
pub(super) fn plan(
    mode: ArchMode,
    candidates: &[Candidate],
    io: &mut PromptIo<'_>,
) -> Result<ArchPlan> {
    let mut plan = ArchPlan::default();
    for candidate in candidates {
        let style = match mode {
            ArchMode::Explicit(style) => Some(style),
            ArchMode::Auto => decide_auto(candidate, io, &mut plan.hints)?,
        };
        let Some(style) = style else {
            continue;
        };
        match &candidate.subrepo {
            Some(name) => plan.subrepos.push((name.clone(), style)),
            None => plan.root = Some(style),
        }
    }
    Ok(plan)
}

fn decide_auto(
    candidate: &Candidate,
    io: &mut PromptIo<'_>,
    hints: &mut Vec<String>,
) -> Result<Option<ArchitectureStyle>> {
    if candidate.existing != ArchitectureStyle::None {
        return Ok(None);
    }
    let suggestion = detect_style(&candidate.source_dir);
    if suggestion.style == ArchitectureStyle::None {
        return Ok(None);
    }
    if !io.interactive {
        hints.push(format!(
            "{}: looks {}; enforce with `anvil architecture set {}{}`",
            candidate.label,
            suggestion.describe(),
            suggestion.style,
            candidate.subrepo.as_ref().map(|n| format!(" --subrepo {n}")).unwrap_or_default()
        ));
        return Ok(None);
    }
    let chosen = if io.use_tui {
        ask_tui(&candidate.label, &suggestion)?
    } else {
        ask_text(&candidate.label, &suggestion, io.reader)?
    };
    Ok((chosen != ArchitectureStyle::None).then_some(chosen))
}

fn default_index(style: ArchitectureStyle) -> usize {
    ArchitectureStyle::ALL.iter().position(|s| *s == style).unwrap_or(0)
}

fn ask_tui(label: &str, suggestion: &Suggestion) -> Result<ArchitectureStyle> {
    let options: Vec<SelectOption> =
        ArchitectureStyle::ALL.iter().map(|s| SelectOption::new(s.as_str(), s.label())).collect();
    let sub = format!("Detected {}", suggestion.describe());
    match prompt::select(
        &format!("Which architecture does {label} follow?"),
        "Architecture",
        Some(&sub),
        &options,
        default_index(suggestion.style),
    )? {
        PromptOutcome::Confirmed(idx) => {
            Ok(ArchitectureStyle::ALL.get(idx).copied().unwrap_or(suggestion.style))
        }
        PromptOutcome::Cancelled => Err(ExitCodeError::silent(130).into()),
    }
}

fn ask_text(
    label: &str,
    suggestion: &Suggestion,
    reader: &mut dyn BufRead,
) -> Result<ArchitectureStyle> {
    println!("Detected {} in {label}.", suggestion.describe());
    println!("Which architecture should Anvil enforce?");
    for (idx, style) in ArchitectureStyle::ALL.iter().enumerate() {
        println!("  [{idx}] {style} ({})", style.label());
    }
    let default = default_index(suggestion.style);
    print!("Select architecture [0-{}] (default: {default}): ", ArchitectureStyle::ALL.len() - 1);
    io::stdout().flush()?;
    let mut answer = String::new();
    let _ = reader.read_line(&mut answer)?;
    let answer = answer.trim();
    let by_number = answer.parse::<usize>().ok().and_then(|n| ArchitectureStyle::ALL.get(n));
    Ok(by_number
        .copied()
        .or_else(|| answer.parse::<ArchitectureStyle>().ok().filter(|_| !answer.is_empty()))
        .unwrap_or(suggestion.style))
}

/// Records the plan in `config` (saving it and refreshing the agent files) and returns the summary line.
pub(super) fn apply(dir: &Path, config: &mut AnvilConfig, plan: &ArchPlan) -> Result<String> {
    let mut changed = false;
    if let Some(style) = plan.root {
        config.set_architecture(style, None, None)?;
        changed = true;
    }
    for (name, style) in &plan.subrepos {
        config.set_architecture(*style, Some(name), None)?;
        changed = true;
    }
    if changed {
        config.save(dir)?;
        sync::sync_repo_with_adapter(dir, None)?;
    }
    Ok(summary(config, plan))
}

fn summary(config: &AnvilConfig, plan: &ArchPlan) -> String {
    let configured: Vec<String> = config
        .architecture_targets()
        .iter()
        .map(|t| format!("{} in {} (source {})", t.setting.style, t.label(), t.source_dir()))
        .collect();
    let mut parts = if configured.is_empty() { vec!["none".to_owned()] } else { configured };
    parts.extend(plan.hints.iter().cloned());
    parts.join("; ")
}
