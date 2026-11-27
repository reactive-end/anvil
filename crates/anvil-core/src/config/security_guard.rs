//! `security_guard` section of `.harness/config.json`: per-category decisions and custom patterns.

use anyhow::{Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};

/// What the guard does when a command falls in a risk category or matches a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GuardDecision {
    /// Prompt the user before the tool call runs.
    Ask,
    /// Refuse the tool call.
    Deny,
    /// Let the tool call through silently.
    Allow,
}

impl GuardDecision {
    /// Every decision, in the order shown to users.
    pub const ALL: [Self; 3] = [Self::Ask, Self::Deny, Self::Allow];

    /// Returns the config spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Deny => "deny",
            Self::Allow => "allow",
        }
    }

    /// Parses `ask`, `deny`, or `allow` (case-insensitive).
    pub fn parse(raw: &str) -> Result<Self> {
        let name = raw.trim().to_ascii_lowercase();
        match Self::ALL.into_iter().find(|d| d.as_str() == name) {
            Some(decision) => Ok(decision),
            None => bail!("unsupported decision `{raw}` (use ask, deny, or allow)"),
        }
    }
}

/// Kind of risk a command carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GuardCategory {
    Git,
    Filesystem,
    Disk,
    Database,
    /// `aws`, `gcloud`, `gsutil`, `az`.
    Cloud,
    /// `terraform`, `pulumi`, `cdk`, `serverless`, `sam`, `helm`, `kubectl`.
    Infra,
    /// `docker` and `podman`.
    Containers,
    Secrets,
    /// Commands whose effect cannot be inspected (downloaded scripts, encoded payloads).
    Opaque,
}

impl GuardCategory {
    /// Every category, in the order shown to users.
    pub const ALL: [Self; 9] = [
        Self::Git,
        Self::Filesystem,
        Self::Disk,
        Self::Database,
        Self::Cloud,
        Self::Infra,
        Self::Containers,
        Self::Secrets,
        Self::Opaque,
    ];

    /// Returns the config spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Filesystem => "filesystem",
            Self::Disk => "disk",
            Self::Database => "database",
            Self::Cloud => "cloud",
            Self::Infra => "infra",
            Self::Containers => "containers",
            Self::Secrets => "secrets",
            Self::Opaque => "opaque",
        }
    }

    /// Parses a category name (case-insensitive).
    pub fn parse(raw: &str) -> Result<Self> {
        let name = raw.trim().to_ascii_lowercase();
        match Self::ALL.into_iter().find(|c| c.as_str() == name) {
            Some(category) => Ok(category),
            None => {
                let valid: Vec<_> = Self::ALL.iter().map(|c| c.as_str()).collect();
                bail!("unsupported category `{raw}` (use {})", valid.join(", "))
            }
        }
    }

    /// Decision applied when the config does not say otherwise.
    pub fn default_decision(self) -> GuardDecision {
        match self {
            Self::Cloud | Self::Infra | Self::Disk => GuardDecision::Deny,
            _ => GuardDecision::Ask,
        }
    }
}

/// Decision per risk category; categories missing from the file keep their default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuardCategories {
    pub git: GuardDecision,
    pub filesystem: GuardDecision,
    pub disk: GuardDecision,
    pub database: GuardDecision,
    pub cloud: GuardDecision,
    pub infra: GuardDecision,
    pub containers: GuardDecision,
    pub secrets: GuardDecision,
    pub opaque: GuardDecision,
}

impl Default for GuardCategories {
    fn default() -> Self {
        let of = GuardCategory::default_decision;
        Self {
            git: of(GuardCategory::Git),
            filesystem: of(GuardCategory::Filesystem),
            disk: of(GuardCategory::Disk),
            database: of(GuardCategory::Database),
            cloud: of(GuardCategory::Cloud),
            infra: of(GuardCategory::Infra),
            containers: of(GuardCategory::Containers),
            secrets: of(GuardCategory::Secrets),
            opaque: of(GuardCategory::Opaque),
        }
    }
}

impl GuardCategories {
    /// Returns the decision for `category`.
    pub fn get(&self, category: GuardCategory) -> GuardDecision {
        match category {
            GuardCategory::Git => self.git,
            GuardCategory::Filesystem => self.filesystem,
            GuardCategory::Disk => self.disk,
            GuardCategory::Database => self.database,
            GuardCategory::Cloud => self.cloud,
            GuardCategory::Infra => self.infra,
            GuardCategory::Containers => self.containers,
            GuardCategory::Secrets => self.secrets,
            GuardCategory::Opaque => self.opaque,
        }
    }

    /// Sets the decision for `category`.
    pub fn set(&mut self, category: GuardCategory, decision: GuardDecision) {
        let slot = match category {
            GuardCategory::Git => &mut self.git,
            GuardCategory::Filesystem => &mut self.filesystem,
            GuardCategory::Disk => &mut self.disk,
            GuardCategory::Database => &mut self.database,
            GuardCategory::Cloud => &mut self.cloud,
            GuardCategory::Infra => &mut self.infra,
            GuardCategory::Containers => &mut self.containers,
            GuardCategory::Secrets => &mut self.secrets,
            GuardCategory::Opaque => &mut self.opaque,
        };
        *slot = decision;
    }
}

/// Which custom pattern list a regex belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternList {
    Deny,
    Ask,
    Allow,
}

impl PatternList {
    /// The list that holds patterns resolving to `decision`.
    pub fn for_decision(decision: GuardDecision) -> Self {
        match decision {
            GuardDecision::Deny => Self::Deny,
            GuardDecision::Ask => Self::Ask,
            GuardDecision::Allow => Self::Allow,
        }
    }

    /// Returns the config key of the list.
    pub fn key(self) -> &'static str {
        match self {
            Self::Deny => "deny_patterns",
            Self::Ask => "ask_patterns",
            Self::Allow => "allow_patterns",
        }
    }
}

/// Fails with the regex error when `pattern` is not a valid regular expression.
pub fn check_pattern(pattern: &str) -> Result<()> {
    match Regex::new(pattern) {
        Ok(_) => Ok(()),
        Err(err) => bail!("invalid regex `{pattern}`: {}", regex_error_line(&err)),
    }
}

fn regex_error_line(err: &regex::Error) -> String {
    err.to_string().lines().last().unwrap_or_default().trim().to_owned()
}

/// A configured pattern that does not compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidPattern {
    pub list: PatternList,
    pub pattern: String,
    pub error: String,
}

/// Configurable security guard for `PreToolUse`.
///
/// Precedence: `deny_patterns`, then `allow_patterns`, then `ask_patterns`, then the category decision.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityGuardConfig {
    pub categories: GuardCategories,
    pub deny_patterns: Vec<String>,
    pub ask_patterns: Vec<String>,
    pub allow_patterns: Vec<String>,
}

impl SecurityGuardConfig {
    /// Returns the decision for `category`.
    pub fn decision(&self, category: GuardCategory) -> GuardDecision {
        self.categories.get(category)
    }

    /// Returns the pattern list stored under `list`.
    pub fn patterns(&self, list: PatternList) -> &[String] {
        match list {
            PatternList::Deny => &self.deny_patterns,
            PatternList::Ask => &self.ask_patterns,
            PatternList::Allow => &self.allow_patterns,
        }
    }

    /// Mutable access to the pattern list stored under `list`.
    pub fn patterns_mut(&mut self, list: PatternList) -> &mut Vec<String> {
        match list {
            PatternList::Deny => &mut self.deny_patterns,
            PatternList::Ask => &mut self.ask_patterns,
            PatternList::Allow => &mut self.allow_patterns,
        }
    }

    /// First valid pattern of `list` that matches any of `texts`; invalid regexes are ignored.
    pub fn first_match(&self, list: PatternList, texts: &[&str]) -> Option<&str> {
        self.patterns(list)
            .iter()
            .filter_map(|p| Regex::new(p).ok().map(|re| (p, re)))
            .find(|(_, re)| texts.iter().any(|t| re.is_match(t)))
            .map(|(p, _)| p.as_str())
    }

    /// Every configured pattern that fails to compile.
    pub fn invalid_patterns(&self) -> Vec<InvalidPattern> {
        [PatternList::Deny, PatternList::Ask, PatternList::Allow]
            .into_iter()
            .flat_map(|list| {
                self.patterns(list).iter().filter_map(move |pattern| {
                    Regex::new(pattern).err().map(|err| InvalidPattern {
                        list,
                        pattern: pattern.clone(),
                        error: regex_error_line(&err),
                    })
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_deny_cloud_infra_disk_and_ask_everything_else() {
        let cfg = SecurityGuardConfig::default();
        for category in GuardCategory::ALL {
            let expected = match category {
                GuardCategory::Cloud | GuardCategory::Infra | GuardCategory::Disk => {
                    GuardDecision::Deny
                }
                _ => GuardDecision::Ask,
            };
            assert_eq!(cfg.decision(category), expected, "{}", category.as_str());
        }
    }

    #[test]
    fn partial_json_keeps_defaults_for_missing_keys() {
        let cfg: SecurityGuardConfig =
            serde_json::from_str(r#"{"categories":{"cloud":"allow"},"deny_patterns":["x"]}"#)
                .expect("parse");
        assert_eq!(cfg.decision(GuardCategory::Cloud), GuardDecision::Allow);
        assert_eq!(cfg.decision(GuardCategory::Infra), GuardDecision::Deny);
        assert_eq!(cfg.decision(GuardCategory::Git), GuardDecision::Ask);
        assert_eq!(cfg.deny_patterns, ["x"]);
        let empty: SecurityGuardConfig = serde_json::from_str("{}").expect("parse empty");
        assert_eq!(empty, SecurityGuardConfig::default());
    }

    #[test]
    fn set_and_get_round_trip_for_every_category() {
        let mut cats = GuardCategories::default();
        for category in GuardCategory::ALL {
            cats.set(category, GuardDecision::Allow);
            assert_eq!(cats.get(category), GuardDecision::Allow);
        }
    }

    #[test]
    fn parses_names_and_rejects_unknown_ones() {
        assert_eq!(GuardDecision::parse(" DENY ").expect("deny"), GuardDecision::Deny);
        assert!(GuardDecision::parse("block").is_err());
        assert_eq!(GuardCategory::parse("Cloud").expect("cloud"), GuardCategory::Cloud);
        assert!(GuardCategory::parse("network").is_err());
    }

    #[test]
    fn invalid_patterns_are_reported_and_skipped_when_matching() {
        let cfg = SecurityGuardConfig {
            deny_patterns: vec!["([unclosed".to_owned(), "terraform\\s+apply".to_owned()],
            ..SecurityGuardConfig::default()
        };
        let invalid = cfg.invalid_patterns();
        assert_eq!(invalid.len(), 1);
        assert_eq!(invalid[0].pattern, "([unclosed");
        assert_eq!(invalid[0].list, PatternList::Deny);
        assert_eq!(
            cfg.first_match(PatternList::Deny, &["terraform  apply -auto-approve"]),
            Some("terraform\\s+apply")
        );
        assert_eq!(cfg.first_match(PatternList::Deny, &["ls"]), None);
        assert!(check_pattern("([unclosed").is_err());
        assert!(check_pattern("^aws\\s+s3 rb").is_ok());
    }

    #[test]
    fn decisions_map_to_their_pattern_lists() {
        for decision in GuardDecision::ALL {
            let list = PatternList::for_decision(decision);
            assert_eq!(list.key(), format!("{}_patterns", decision.as_str()));
        }
    }
}
