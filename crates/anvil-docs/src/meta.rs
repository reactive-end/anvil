//! Typed access to the OKF frontmatter fields of a concept.

use chrono::{DateTime, FixedOffset};

use crate::frontmatter::Document;
use crate::yaml::{self, Mapping, Yaml};
use crate::{KEY_PATHS, KEY_SYNCED_COMMIT, KEY_SYNCED_DIGEST};

/// An actor and the moment it acted: the shape of `generated` and of each `verified` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    pub by: String,
    pub at: String,
}

impl Stamp {
    /// Parsed `at`, or `None` when it is not RFC 3339.
    pub fn instant(&self) -> Option<DateTime<FixedOffset>> {
        DateTime::parse_from_rfc3339(&self.at).ok()
    }

    /// `true` for `human:<name>` actors.
    pub fn is_human(&self) -> bool {
        self.by.starts_with("human:")
    }

    /// Frontmatter representation.
    pub fn to_value(&self) -> Yaml {
        let mut map = Mapping::new();
        yaml::set(&mut map, "by", yaml::string(&self.by));
        yaml::set(&mut map, "at", yaml::string(&self.at));
        Yaml::Hash(map)
    }

    fn from_value(value: &Yaml) -> Option<Self> {
        let map = value.as_hash()?;
        let text = |key: &str| yaml::get(map, key).and_then(Yaml::as_str).map(str::to_owned);
        Some(Self { by: text("by")?, at: text("at")? })
    }
}

/// Declared lifecycle state of a concept.
pub const STATUSES: &[&str] = &["draft", "stable", "deprecated"];

impl Document {
    /// A frontmatter field of any type.
    pub fn field(&self, key: &str) -> Option<&Yaml> {
        yaml::get(self.meta()?, key)
    }

    /// A string field of the frontmatter.
    pub fn str_field(&self, key: &str) -> Option<&str> {
        self.field(key)?.as_str()
    }

    /// Required OKF `type`.
    pub fn concept_type(&self) -> Option<&str> {
        self.str_field("type")
    }

    /// Human-readable title.
    pub fn title(&self) -> Option<&str> {
        self.str_field("title")
    }

    /// One-line description.
    pub fn description(&self) -> Option<&str> {
        self.str_field("description")
    }

    /// Lifecycle state (`draft`, `stable`, `deprecated`).
    pub fn status(&self) -> Option<&str> {
        self.str_field("status")
    }

    /// Free-form tags.
    pub fn tags(&self) -> Vec<String> {
        self.string_list("tags")
    }

    /// Repo-relative globs of the code the concept documents.
    pub fn code_paths(&self) -> Vec<String> {
        self.string_list(KEY_PATHS)
    }

    /// Commit the concept was last synced to.
    pub fn synced_commit(&self) -> Option<&str> {
        self.str_field(KEY_SYNCED_COMMIT).filter(|c| !c.is_empty())
    }

    /// Fingerprint of the uncommitted code changes present at the last sync.
    pub fn synced_digest(&self) -> Option<&str> {
        self.str_field(KEY_SYNCED_DIGEST).filter(|c| !c.is_empty())
    }

    /// Latest `stale_after` deadline, when present and valid.
    pub fn stale_after(&self) -> Option<DateTime<FixedOffset>> {
        DateTime::parse_from_rfc3339(self.str_field("stale_after")?).ok()
    }

    /// Who generated the content last, and when.
    pub fn generated(&self) -> Option<Stamp> {
        Stamp::from_value(self.field("generated")?)
    }

    /// Verification records in file order; malformed entries are skipped.
    pub fn verified(&self) -> Vec<Stamp> {
        let Some(Yaml::Array(items)) = self.field("verified") else {
            return Vec::new();
        };
        items.iter().filter_map(Stamp::from_value).collect()
    }

    fn string_list(&self, key: &str) -> Vec<String> {
        match self.field(key) {
            Some(Yaml::Array(items)) => {
                items.iter().filter_map(Yaml::as_str).map(str::to_owned).collect()
            }
            Some(Yaml::String(one)) => vec![one.clone()],
            _ => Vec::new(),
        }
    }

    /// Sets a string field, keeping its position when it already exists.
    pub fn set_str(&mut self, key: &str, value: &str) {
        yaml::set(self.meta_mut(), key, yaml::string(value));
    }

    /// Removes a field.
    pub fn remove_field(&mut self, key: &str) {
        yaml::remove(self.meta_mut(), key);
    }

    /// Replaces `generated`.
    pub fn set_generated(&mut self, stamp: &Stamp) {
        yaml::set(self.meta_mut(), "generated", stamp.to_value());
    }

    /// Appends a verification record.
    pub fn push_verified(&mut self, stamp: &Stamp) {
        let meta = self.meta_mut();
        if let Some(Yaml::Array(items)) = yaml::get_mut(meta, "verified") {
            items.push(stamp.to_value());
            return;
        }
        yaml::set(meta, "verified", Yaml::Array(vec![stamp.to_value()]));
    }
}
