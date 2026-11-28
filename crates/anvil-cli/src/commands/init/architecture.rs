//! `anvil init` step that records the architecture style of each project.
//!
//! `--architecture <style>` is applied as given. With `auto`, the style is detected from the folder
//! names and, when one is found, the user picks it interactively (default: the detected one); without a
//! prompt it is only suggested, because enforcing a guessed style could fail builds unexpectedly.
//!
//! In a monorepo an explicit style only goes to the subrepos it fits: a client app (Vite, React, Vue,
//! Angular or Svelte with no server framework) is skipped with a hint, a style already chosen for a
//! subrepo is kept, and the repository root gets none when subrepos carry their own.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use anvil_adapters::sync;
use anvil_core::config::architecture::detect::{Suggestion, client_app_stack, detect_style};
use anvil_core::config::architecture::{ArchitectureStyle, DEFAULT_SOURCE_ROOT};
use anvil_core::config::{AnvilConfig, StackKind};
use anyhow::Result;

use crate::commands::ExitCodeError;
use crate::ui::prompt::{self, PromptOutcome, SelectOption};

/// The `--architecture` flag.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ArchMode {
    /// Detect, then prompt (interactive) or only suggest.
    Auto,
    /// Apply this style to every project it fits (see the module docs).
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
    /// The UI stack when the project is a client app with no server framework.
    client_app: Option<StackKind>,
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
            client_app: None,
        })
    }

    /// A subrepo `name` at `dir`, recorded in the workspace root config `root_config`.
    pub(super) fn subrepo(
        label: &str,
        name: &str,
        dir: &Path,
        stacks: &[StackKind],
        root_config: &AnvilConfig,
    ) -> Self {
        let existing = root_config.architecture.subrepos.get(name).map(|s| s.style);
        Self {
            label: label.to_owned(),
            subrepo: Some(name.to_owned()),
            source_dir: dir.join(DEFAULT_SOURCE_ROOT),
            existing: existing.unwrap_or_default(),
            client_app: client_app_stack(stacks),
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

/// Decides the style of every candidate. Subrepos come first: the root only gets a style when no
/// subrepo has one, so a monorepo never states the same rules for the root and for its subrepos.
pub(super) fn plan(
    mode: ArchMode,
    candidates: &[Candidate],
    io: &mut PromptIo<'_>,
) -> Result<ArchPlan> {
    let mut plan = ArchPlan::default();
    let subrepos = candidates.iter().filter(|c| c.subrepo.is_some());
    let roots = candidates.iter().filter(|c| c.subrepo.is_none());
    for candidate in subrepos.chain(roots) {
        if candidate.subrepo.is_none() && subrepos_own_the_style(candidates, &plan) {
            continue;
        }
        let style = match mode {
            ArchMode::Explicit(style) => decide_explicit(candidate, style, &mut plan.hints),
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

/// `true` when a subrepo already has a style in the config or was just given one.
fn subrepos_own_the_style(candidates: &[Candidate], plan: &ArchPlan) -> bool {
    !plan.subrepos.is_empty()
        || candidates.iter().any(|c| c.subrepo.is_some() && c.existing != ArchitectureStyle::None)
}

/// Applies the `--architecture` style to `candidate` when it fits; a subrepo keeps its own choice and
/// a client app is skipped (feature-sliced design is the one style built for them).
fn decide_explicit(
    candidate: &Candidate,
    style: ArchitectureStyle,
    hints: &mut Vec<String>,
) -> Option<ArchitectureStyle> {
    let Some(name) = &candidate.subrepo else {
        return Some(style);
    };
    if style == ArchitectureStyle::None {
        return Some(style);
    }
    if candidate.existing != ArchitectureStyle::None {
        return None;
    }
    if let Some(stack) = candidate.client_app
        && style != ArchitectureStyle::FeatureSliced
    {
        hints.push(format!(
            "{}: {} client app (no server framework), no {style} rules applied; set one explicitly with `anvil architecture set <style> --subrepo {name}`",
            candidate.label,
            format!("{stack:?}").to_ascii_lowercase()
        ));
        return None;
    }
    Some(style)
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
