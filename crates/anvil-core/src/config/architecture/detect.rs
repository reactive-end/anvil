//! Suggests an architecture style from the folder names under a source root.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::ArchitectureStyle;
use crate::config::StackKind;

const INFRA: &[&str] = &["infrastructure", "infraestructure", "infra", "adapters", "persistence"];
const USE_CASES: &[&str] = &["use-cases", "usecases", "use_cases"];
const FSD_LAYERS: &[&str] =
    &["app", "processes", "pages", "widgets", "features", "entities", "shared"];

/// How sure a suggestion is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

/// A suggested style with the evidence behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    pub style: ArchitectureStyle,
    pub confidence: Confidence,
    pub note: String,
}

impl Suggestion {
    fn new(style: ArchitectureStyle, confidence: Confidence, note: impl Into<String>) -> Self {
        Self { style, confidence, note: note.into() }
    }

    /// `hexagonal (high confidence): 2 of 3 modules have ...`.
    pub fn describe(&self) -> String {
        format!("{} ({} confidence): {}", self.style, self.confidence.as_str(), self.note)
    }
}

/// Browser UI stacks: a project built only on these is a client app, not a service.
const CLIENT_STACKS: &[StackKind] =
    &[StackKind::Vite, StackKind::React, StackKind::Vue, StackKind::Angular, StackKind::Svelte];
/// Stacks that run server code, which make a project more than a client app.
const SERVER_STACKS: &[StackKind] = &[
    StackKind::NestJs,
    StackKind::NextJs,
    StackKind::Astro,
    StackKind::Laravel,
    StackKind::Php,
    StackKind::Python,
    StackKind::Rust,
    StackKind::KotlinAndroid,
];

/// The client-app stack that identifies a project (Vite, React, Vue, Angular or Svelte) when no server
/// framework is detected next to it; `None` for services and for projects of an unknown kind.
pub fn client_app_stack(stacks: &[StackKind]) -> Option<StackKind> {
    if stacks.iter().any(|s| SERVER_STACKS.contains(s)) {
        return None;
    }
    CLIENT_STACKS.iter().copied().find(|s| stacks.contains(s))
}

/// Lower-cased names of the directories directly under a unit (the source root or one module).
type Names = BTreeSet<String>;

fn child_dirs(dir: &Path) -> Names {
    let Ok(entries) = fs::read_dir(dir) else {
        return Names::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().to_ascii_lowercase())
        .filter(|name| !name.starts_with('.') && name != "node_modules")
        .collect()
}

fn has_any(names: &Names, wanted: &[&str]) -> bool {
    wanted.iter().any(|w| names.contains(*w))
}

fn is_hexagonal(names: &Names) -> bool {
    names.contains("domain") && names.contains("application") && has_any(names, INFRA)
}

fn is_clean(names: &Names) -> bool {
    has_any(names, &["entities", "domain"]) && has_any(names, USE_CASES)
}

fn is_layered(names: &Names) -> bool {
    has_any(names, &["controller", "controllers"])
        && has_any(names, &["service", "services"])
        && has_any(names, &["repository", "repositories"])
}

/// Modules: the directories under `<source>/modules` when it exists, else those under `<source>`.
fn modules(source: &Path, top: &Names) -> Vec<(String, Names)> {
    let base = if top.contains("modules") { source.join("modules") } else { source.to_path_buf() };
    child_dirs(&base)
        .into_iter()
        .map(|name| {
            let children = child_dirs(&base.join(&name));
            (name, children)
        })
        .filter(|(_, children)| !children.is_empty())
        .collect()
}

fn count_matching(units: &[(String, Names)], test: fn(&Names) -> bool) -> usize {
    units.iter().filter(|(_, names)| test(names)).count()
}

/// Suggests a style for the source directory `source`.
pub fn detect_style(source: &Path) -> Suggestion {
    if !source.is_dir() {
        let note = format!("source root `{}` does not exist", source.display());
        return Suggestion::new(ArchitectureStyle::None, Confidence::Low, note);
    }
    let top = child_dirs(source);
    let units = modules(source, &top);
    feature_sliced(&top)
        .or_else(|| clean(&top, &units))
        .or_else(|| hexagonal(&top, &units))
        .or_else(|| layered(&top, &units))
        .unwrap_or_else(|| {
            Suggestion::new(
                ArchitectureStyle::None,
                Confidence::Low,
                "no recognizable layer or module folder pattern",
            )
        })
}

fn feature_sliced(top: &Names) -> Option<Suggestion> {
    let found = FSD_LAYERS.iter().filter(|l| top.contains(**l)).count();
    if !(top.contains("features") && top.contains("entities") && top.contains("shared")) {
        return None;
    }
    let confidence = if found >= 4 { Confidence::High } else { Confidence::Medium };
    let note = format!("{found} of {} feature-sliced layers present", FSD_LAYERS.len());
    Some(Suggestion::new(ArchitectureStyle::FeatureSliced, confidence, note))
}

fn clean(top: &Names, units: &[(String, Names)]) -> Option<Suggestion> {
    if is_clean(top) {
        let ring = has_any(top, &["adapters", "interface-adapters", "controllers", "presenters"]);
        let confidence = if ring { Confidence::High } else { Confidence::Medium };
        return Some(Suggestion::new(
            ArchitectureStyle::Clean,
            confidence,
            "entities/domain and use-cases folders at the source root",
        ));
    }
    let hits = count_matching(units, is_clean);
    (hits > 0 && hits * 2 >= units.len()).then(|| {
        let note = format!("{hits} of {} modules have entities/domain and use-cases", units.len());
        Suggestion::new(ArchitectureStyle::Clean, Confidence::Medium, note)
    })
}

fn hexagonal(top: &Names, units: &[(String, Names)]) -> Option<Suggestion> {
    let hits = count_matching(units, is_hexagonal);
    if hits > 0 && hits * 2 >= units.len() {
        let confidence = if hits >= 2 { Confidence::High } else { Confidence::Medium };
        let note = format!(
            "{hits} of {} modules have domain + application + infrastructure folders",
            units.len()
        );
        return Some(Suggestion::new(ArchitectureStyle::Hexagonal, confidence, note));
    }
    is_hexagonal(top).then(|| {
        Suggestion::new(
            ArchitectureStyle::Hexagonal,
            Confidence::Medium,
            "domain + application + infrastructure folders at the source root",
        )
    })
}

fn layered(top: &Names, units: &[(String, Names)]) -> Option<Suggestion> {
    if is_layered(top) {
        return Some(Suggestion::new(
            ArchitectureStyle::Layered,
            Confidence::High,
            "controllers + services + repositories folders at the source root",
        ));
    }
    let hits = count_matching(units, is_layered);
    (hits > 0 && hits * 2 >= units.len()).then(|| {
        let confidence = if hits >= 2 { Confidence::High } else { Confidence::Medium };
        let note =
            format!("{hits} of {} modules have controllers + services + repositories", units.len());
        Suggestion::new(ArchitectureStyle::Layered, confidence, note)
    })
}

#[cfg(test)]
mod tests;
