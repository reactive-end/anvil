//! Architecture style presets (`hexagonal`, `clean`, ...) recorded per repository or subrepo.
//!
//! The chosen style lives in `.harness/config.json` under `architecture`; it is expanded into import
//! rules ([`presets::expand`]) when checks run, never written over the user's own `boundaries`, and
//! stated to agents through [`guidance::architecture_lines`].
//!
//! Per-subrepo choices live in `architecture.subrepos`, a map keyed by subrepo name, rather than on
//! `SubrepoConfig`: stack detection rebuilds the `subrepos` list on every run, so anything stored there
//! would be lost, while this map is never touched by detection.

pub mod detect;
pub mod guidance;
pub mod presets;
pub mod rules;

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::AnvilConfig;
pub use rules::{ArchRule, ArchRuleKind, ModuleIsolation};

/// Default source directory the presets are anchored to, relative to the repository or subrepo.
pub const DEFAULT_SOURCE_ROOT: &str = "src";

/// An architecture style whose rules Anvil can generate and enforce.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum ArchitectureStyle {
    #[default]
    None,
    Hexagonal,
    Clean,
    Layered,
    FeatureSliced,
    ModularMonolith,
    EventDriven,
}

impl ArchitectureStyle {
    /// Every style, `none` first, in the order prompts list them.
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Hexagonal,
        Self::Clean,
        Self::Layered,
        Self::FeatureSliced,
        Self::ModularMonolith,
        Self::EventDriven,
    ];

    /// Canonical kebab-case name used in the config and on the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Hexagonal => "hexagonal",
            Self::Clean => "clean",
            Self::Layered => "layered",
            Self::FeatureSliced => "feature-sliced",
            Self::ModularMonolith => "modular-monolith",
            Self::EventDriven => "event-driven",
        }
    }

    /// Short human description shown in prompts and agent instructions.
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "no architecture rules",
            Self::Hexagonal => "ports & adapters",
            Self::Clean => "clean architecture",
            Self::Layered => "layered",
            Self::FeatureSliced => "feature-sliced design",
            Self::ModularMonolith => "modular monolith",
            Self::EventDriven => "events & handlers",
        }
    }
}

impl fmt::Display for ArchitectureStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ArchitectureStyle {
    type Err = anyhow::Error;

    fn from_str(raw: &str) -> Result<Self> {
        let key = raw.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        Ok(match key.as_str() {
            "none" | "off" | "" => Self::None,
            "hexagonal" | "ports-and-adapters" | "ports-adapters" => Self::Hexagonal,
            "clean" | "clean-architecture" | "onion" => Self::Clean,
            "layered" | "n-tier" | "mvc" => Self::Layered,
            "feature-sliced" | "fsd" | "feature-sliced-design" => Self::FeatureSliced,
            "modular-monolith" | "modular" | "modules" => Self::ModularMonolith,
            "event-driven" | "events" | "eda" => Self::EventDriven,
            other => bail!(
                "unknown architecture style `{other}` (expected none|hexagonal|clean|layered|feature-sliced|modular-monolith|event-driven)"
            ),
        })
    }
}

/// The architecture choice of one repository or subrepo.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ArchitectureSetting {
    #[serde(default)]
    pub style: ArchitectureStyle,
    /// Directory the rules are anchored to, relative to the repository or subrepo (default `src`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_root: Option<String>,
    /// Restricts module isolation to these module directory names (default: every directory).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
    /// Hexagonal only: also forbid every `@nestjs/*` import in `application/` (default `false`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict_framework: Option<bool>,
}

/// The `architecture` section of `.harness/config.json`: the repo-level choice plus per-subrepo ones.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ArchitectureConfig {
    #[serde(flatten)]
    pub repo: ArchitectureSetting,
    /// Choices of subrepos, keyed by `SubrepoConfig::name`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub subrepos: BTreeMap<String, ArchitectureSetting>,
}

impl ArchitectureConfig {
    /// `true` when nothing is configured, so the section is left out of the saved config.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

static NO_SETTING: ArchitectureSetting = ArchitectureSetting {
    style: ArchitectureStyle::None,
    source_root: None,
    modules: Vec::new(),
    strict_framework: None,
};

/// A repository root or subrepo together with its architecture setting.
#[derive(Debug, Clone, Copy)]
pub struct ArchTarget<'a> {
    /// Subrepo name, or `None` for the repository root.
    pub subrepo: Option<&'a str>,
    /// Subrepo path relative to the repository root (`.` for the root itself).
    pub path: &'a str,
    pub setting: &'a ArchitectureSetting,
}

impl ArchTarget<'_> {
    /// Name used in messages: the subrepo name or `this repository`.
    pub fn label(&self) -> &str {
        self.subrepo.unwrap_or("this repository")
    }

    /// Repo-relative source directory (`src`, or `backend/src` for a subrepo at `backend`).
    pub fn source_dir(&self) -> String {
        let root =
            normalize_dir(self.setting.source_root.as_deref().unwrap_or(DEFAULT_SOURCE_ROOT));
        let base = normalize_dir(self.path);
        match (base.as_str(), root.as_str()) {
            (".", ".") => String::new(),
            (".", _) => root,
            (_, ".") => base,
            _ => format!("{base}/{root}"),
        }
    }
}

fn normalize_dir(raw: &str) -> String {
    let slashed = raw.trim().replace('\\', "/");
    let trimmed = slashed.trim_start_matches("./").trim_matches('/');
    if trimmed.is_empty() { ".".to_owned() } else { trimmed.to_owned() }
}

impl AnvilConfig {
    /// Every target (repository root plus configured subrepos), including those set to `none`.
    pub fn architecture_all_targets(&self) -> Vec<ArchTarget<'_>> {
        let mut targets =
            vec![ArchTarget { subrepo: None, path: ".", setting: &self.architecture.repo }];
        for sub in &self.subrepos {
            let setting = self.architecture.subrepos.get(&sub.name).unwrap_or(&NO_SETTING);
            targets.push(ArchTarget { subrepo: Some(&sub.name), path: &sub.path, setting });
        }
        targets
    }

    /// Targets whose style is not `none`.
    pub fn architecture_targets(&self) -> Vec<ArchTarget<'_>> {
        let mut targets = self.architecture_all_targets();
        targets.retain(|t| t.setting.style != ArchitectureStyle::None);
        targets
    }

    /// Rules generated from every configured style; computed on demand, never stored.
    pub fn architecture_rules(&self) -> Vec<ArchRule> {
        self.architecture_targets().iter().flat_map(presets::expand).collect()
    }

    /// Records `style` for the repository root or the subrepo `subrepo`.
    pub fn set_architecture(
        &mut self,
        style: ArchitectureStyle,
        subrepo: Option<&str>,
        source_root: Option<&str>,
    ) -> Result<()> {
        let setting = match subrepo {
            None => &mut self.architecture.repo,
            Some(name) => {
                if !self.subrepos.iter().any(|s| s.name == name) {
                    let known: Vec<&str> = self.subrepos.iter().map(|s| s.name.as_str()).collect();
                    bail!("unknown subrepo `{name}` (known: {})", known.join(", "));
                }
                self.architecture.subrepos.entry(name.to_owned()).or_default()
            }
        };
        setting.style = style;
        if let Some(dir) = source_root {
            setting.source_root = Some(normalize_dir(dir)).filter(|d| d != DEFAULT_SOURCE_ROOT);
        }
        if style == ArchitectureStyle::None
            && let Some(name) = subrepo
        {
            self.architecture.subrepos.remove(name);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
