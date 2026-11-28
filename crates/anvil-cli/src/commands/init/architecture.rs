//! `anvil init` step that records the architecture style of each project.
//!
//! `--architecture <style>` is applied as given. With `auto`, the style is detected from the folder
//! names and an interactive run always offers every preset (default: the detected one, else `none`),
//! even for an empty folder; without a prompt it is only suggested, because enforcing a guessed style
//! could fail builds unexpectedly.
//!
//! In a monorepo an explicit style only goes to the subrepos it fits: a client app (Vite, React, Vue,
//! Angular or Svelte with no server framework) is skipped with a hint, a style already chosen for a
//! subrepo is kept, and the repository root gets none when subrepos carry their own.

use std::io::{BufRead, Write};
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
    pub writer: &'a mut dyn Write,
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

/// `--architecture auto`: a project that already has a style keeps it. Otherwise an interactive run
/// always offers every preset (default: the detected style, else `none`); a non-interactive one only
/// suggests the detected style, because enforcing a guessed style could fail builds unexpectedly.
fn decide_auto(
    candidate: &Candidate,
    io: &mut PromptIo<'_>,
    hints: &mut Vec<String>,
) -> Result<Option<ArchitectureStyle>> {
    if candidate.existing != ArchitectureStyle::None {
        return Ok(None);
    }
    let suggestion = detect_style(&candidate.source_dir);
    if !io.interactive {
        if suggestion.style != ArchitectureStyle::None {
            hints.push(format!(
                "{}: looks {}; enforce with `anvil architecture set {}{}`",
                candidate.label,
                suggestion.describe(),
                suggestion.style,
                candidate.subrepo.as_ref().map(|n| format!(" --subrepo {n}")).unwrap_or_default()
            ));
        }
        return Ok(None);
    }
    let note = context_note(candidate, &suggestion);
    let chosen = if io.use_tui {
        ask_tui(&candidate.label, &note, suggestion.style)?
    } else {
        ask_text(&candidate.label, &note, suggestion.style, io.reader, io.writer)?
    };
    if chosen == ArchitectureStyle::None {
        return Ok(None);
    }
    if source_is_empty(&candidate.source_dir) {
        hints.push(format!(
            "{}: no code under `{DEFAULT_SOURCE_ROOT}` yet; {chosen} rules apply once code exists there",
            candidate.label
        ));
    }
    Ok(Some(chosen))
}

/// What init knows about the project, shown under the question.
fn context_note(candidate: &Candidate, suggestion: &Suggestion) -> String {
    if suggestion.style != ArchitectureStyle::None {
        return format!("Detected {}", suggestion.describe());
    }
    if source_is_empty(&candidate.source_dir) {
        return format!("No code under `{DEFAULT_SOURCE_ROOT}` yet, so nothing to detect");
    }
    match candidate.client_app {
        Some(stack) => format!(
            "No style detected; {} client app (no server framework), feature-sliced fits it",
            format!("{stack:?}").to_ascii_lowercase()
        ),
        None => format!("No style detected ({})", suggestion.note),
    }
}

/// `true` when `dir` is missing or has no entries yet.
fn source_is_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir).map(|mut entries| entries.next().is_none()).unwrap_or(true)
}

fn default_index(style: ArchitectureStyle) -> usize {
    ArchitectureStyle::ALL.iter().position(|s| *s == style).unwrap_or(0)
}

fn ask_tui(label: &str, note: &str, default: ArchitectureStyle) -> Result<ArchitectureStyle> {
    let options: Vec<SelectOption> =
        ArchitectureStyle::ALL.iter().map(|s| SelectOption::new(s.as_str(), s.label())).collect();
    match prompt::select(
        &format!("Which architecture does {label} follow?"),
        "Architecture",
        Some(note),
        &options,
        default_index(default),
    )? {
        PromptOutcome::Confirmed(idx) => {
            Ok(ArchitectureStyle::ALL.get(idx).copied().unwrap_or(default))
        }
        PromptOutcome::Cancelled => Err(ExitCodeError::silent(130).into()),
    }
}

fn ask_text(
    label: &str,
    note: &str,
    default: ArchitectureStyle,
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
) -> Result<ArchitectureStyle> {
    writeln!(writer, "{label}: {note}.")?;
    writeln!(writer, "Which architecture should Anvil enforce?")?;
    for (idx, style) in ArchitectureStyle::ALL.iter().enumerate() {
        writeln!(writer, "  [{idx}] {style} ({})", style.label())?;
    }
    write!(
        writer,
        "Select architecture [0-{}] (default: {}): ",
        ArchitectureStyle::ALL.len() - 1,
        default_index(default)
    )?;
    writer.flush()?;
    let mut answer = String::new();
    let _ = reader.read_line(&mut answer)?;
    let answer = answer.trim();
    let by_number = answer.parse::<usize>().ok().and_then(|n| ArchitectureStyle::ALL.get(n));
    Ok(by_number
        .copied()
        .or_else(|| answer.parse::<ArchitectureStyle>().ok().filter(|_| !answer.is_empty()))
        .unwrap_or(default))
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;

    use tempfile::TempDir;

    use super::*;

    /// Runs `plan` in `Auto` mode over `candidates` with scripted `input`; returns the plan and the
    /// text the prompt printed.
    fn run(candidates: &[Candidate], interactive: bool, input: &str) -> (ArchPlan, String) {
        let mut reader = Cursor::new(input.as_bytes().to_vec());
        let mut written = Vec::new();
        let plan = {
            let mut io =
                PromptIo { interactive, use_tui: false, reader: &mut reader, writer: &mut written };
            plan(ArchMode::Auto, candidates, &mut io).expect("plan")
        };
        (plan, String::from_utf8(written).expect("utf8"))
    }

    fn root_candidate(dir: &Path) -> Vec<Candidate> {
        vec![Candidate::root("this project", dir).expect("candidate")]
    }

    fn hexagonal_tree(dir: &Path) {
        for module in ["orders", "customers"] {
            for layer in ["domain", "application", "infrastructure"] {
                let layer_dir = dir.join("src").join(module).join(layer);
                fs::create_dir_all(&layer_dir).expect("mkdir");
                fs::write(layer_dir.join("a.ts"), "export const a = 1;\n").expect("write");
            }
        }
    }

    fn applied_style(dir: &Path, plan: &ArchPlan) -> (ArchitectureStyle, String) {
        let mut config = AnvilConfig::load_or_default(dir).expect("config");
        let line = apply(dir, &mut config, plan).expect("apply");
        (AnvilConfig::load_or_default(dir).expect("reload").architecture.repo.style, line)
    }

    #[test]
    fn empty_folder_offers_every_preset_and_records_the_pick() {
        let dir = TempDir::new().expect("tempdir");
        let (plan, shown) = run(&root_candidate(dir.path()), true, "1\n");
        println!("{shown}");
        for style in ArchitectureStyle::ALL {
            assert!(shown.contains(&format!("{style} ({})", style.label())), "missing {style}");
        }
        assert!(shown.contains("No code under `src` yet"), "{shown}");
        assert!(shown.contains("(default: 0)"), "{shown}");
        let (style, line) = applied_style(dir.path(), &plan);
        assert_eq!(style, ArchitectureStyle::Hexagonal);
        assert!(line.contains("hexagonal"), "{line}");
        assert!(line.contains("rules apply once code exists there"), "{line}");
    }

    #[test]
    fn enter_on_an_empty_folder_keeps_none() {
        let dir = TempDir::new().expect("tempdir");
        let (plan, shown) = run(&root_candidate(dir.path()), true, "\n");
        assert!(shown.contains("Which architecture should Anvil enforce?"), "{shown}");
        assert!(plan.root.is_none() && plan.hints.is_empty());
        let (style, line) = applied_style(dir.path(), &plan);
        assert_eq!(style, ArchitectureStyle::None);
        assert_eq!(line, "none");
    }

    #[test]
    fn a_detected_style_is_preselected() {
        let dir = TempDir::new().expect("tempdir");
        hexagonal_tree(dir.path());
        let (plan, shown) = run(&root_candidate(dir.path()), true, "\n");
        println!("{shown}");
        assert!(shown.contains("Detected hexagonal"), "{shown}");
        assert!(shown.contains("(default: 1)"), "{shown}");
        assert_eq!(plan.root, Some(ArchitectureStyle::Hexagonal));
        assert!(plan.hints.is_empty(), "code exists, so no empty-source hint");
    }

    #[test]
    fn a_client_app_subrepo_defaults_to_none_and_lists_feature_sliced() {
        let dir = TempDir::new().expect("tempdir");
        let candidate = Candidate::subrepo(
            "web",
            "web",
            dir.path(),
            &[StackKind::React],
            &AnvilConfig::default(),
        );
        fs::create_dir_all(dir.path().join("src/components")).expect("mkdir");
        let (plan, shown) = run(&[candidate], true, "\n");
        println!("{shown}");
        assert!(shown.contains("react client app"), "{shown}");
        assert!(shown.contains("feature-sliced (feature-sliced design)"), "{shown}");
        assert!(shown.contains("(default: 0)"), "{shown}");
        assert!(plan.subrepos.is_empty());
    }

    #[test]
    fn non_interactive_never_prompts_and_only_suggests() {
        let empty = TempDir::new().expect("tempdir");
        let (plan, shown) = run(&root_candidate(empty.path()), false, "1\n");
        assert!(shown.is_empty(), "{shown}");
        assert!(plan.root.is_none() && plan.hints.is_empty());

        let detected = TempDir::new().expect("tempdir");
        hexagonal_tree(detected.path());
        let (plan, shown) = run(&root_candidate(detected.path()), false, "1\n");
        assert!(shown.is_empty(), "{shown}");
        assert!(plan.root.is_none());
        assert!(plan.hints[0].contains("anvil architecture set hexagonal"), "{:?}", plan.hints);
    }
}
