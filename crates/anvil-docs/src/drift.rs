//! Drift detection: has the code a concept documents changed since the concept was synced?

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use crate::concept::Concept;
use crate::git;
use crate::paths::PathMatcher;

/// Whether a concept still describes its code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    /// Out of date, with the reason.
    Stale(String),
}

impl Freshness {
    /// `true` for `Stale`.
    pub fn is_stale(&self) -> bool {
        matches!(self, Self::Stale(_))
    }

    /// The reason a concept is stale.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Fresh => None,
            Self::Stale(reason) => Some(reason),
        }
    }
}

/// Evaluates freshness for many concepts while asking git once per distinct commit.
pub struct Drift<'a> {
    root: &'a Path,
    changes: HashMap<String, Option<BTreeSet<String>>>,
}

impl<'a> Drift<'a> {
    pub fn new(root: &'a Path) -> Self {
        Self { root, changes: HashMap::new() }
    }

    fn changes_since(&mut self, commit: &str) -> Option<&BTreeSet<String>> {
        let root = self.root;
        self.changes
            .entry(commit.to_owned())
            .or_insert_with(|| git::changed_since(root, commit))
            .as_ref()
    }

    /// Freshness of `concept`. A concept that declares no `x-anvil-paths` tracks no code and stays fresh.
    pub fn check(&mut self, concept: &Concept) -> Freshness {
        let doc = &concept.doc;
        if let Some(deadline) = doc.stale_after()
            && deadline <= chrono::Utc::now()
        {
            return Freshness::Stale(format!(
                "`stale_after` ({}) has passed",
                deadline.to_rfc3339()
            ));
        }
        let globs = doc.code_paths();
        if globs.is_empty() {
            return Freshness::Fresh;
        }
        let Some(commit) = doc.synced_commit() else {
            return Freshness::Stale("no x-anvil-synced-commit recorded".to_owned());
        };
        let Ok(matcher) = PathMatcher::new(&globs) else {
            return Freshness::Stale("x-anvil-paths holds an invalid glob".to_owned());
        };
        let Some(changed) = self.changes_since(commit) else {
            return Freshness::Stale(format!(
                "synced commit `{commit}` is unknown to this repository"
            ));
        };
        let hits: Vec<String> = changed.iter().filter(|f| matcher.is_match(f)).cloned().collect();
        if hits.is_empty() || doc.synced_digest() == Some(digest(self.root, &hits).as_str()) {
            return Freshness::Fresh;
        }
        Freshness::Stale(describe(&hits, commit))
    }
}

fn describe(hits: &[String], commit: &str) -> String {
    const SHOWN: usize = 3;
    let mut names = hits.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
    if hits.len() > SHOWN {
        names.push_str(&format!(" (+{} more)", hits.len() - SHOWN));
    }
    format!("{} file(s) changed since {commit}: {names}", hits.len())
}

/// Fingerprint of the content of `files` (path and bytes; a missing file counts as deleted).
pub fn digest(root: &Path, files: &[String]) -> String {
    let mut sorted: Vec<&String> = files.iter().collect();
    sorted.sort();
    let mut hasher = blake3::Hasher::new();
    for rel in sorted {
        hasher.update(rel.as_bytes());
        hasher.update(&[0]);
        match fs::read(root.join(rel)) {
            Ok(bytes) => hasher.update(&bytes),
            Err(_) => hasher.update(b"\0deleted"),
        };
        hasher.update(&[0]);
    }
    hasher.finalize().to_hex().as_str()[..16].to_owned()
}

/// The sync state to record for a concept covering `globs`: the `HEAD` commit and the fingerprint of
/// the uncommitted changes to its code (`None` when the code is clean).
pub fn sync_state(root: &Path, globs: &[String]) -> (Option<String>, Option<String>) {
    let Some(head) = git::head_short(root) else {
        return (None, None);
    };
    let Ok(matcher) = PathMatcher::new(globs) else {
        return (Some(head), None);
    };
    let dirty: Vec<String> = git::changed_since(root, "HEAD")
        .unwrap_or_default()
        .into_iter()
        .filter(|f| matcher.is_match(f))
        .collect();
    let fingerprint = (!dirty.is_empty()).then(|| digest(root, &dirty));
    (Some(head), fingerprint)
}

/// Concepts whose code globs match `rel`.
pub fn documenting<'c>(concepts: &'c [Concept], rel: &str) -> Vec<&'c Concept> {
    concepts
        .iter()
        .filter(|c| {
            let globs = c.doc.code_paths();
            !globs.is_empty() && PathMatcher::new(&globs).is_ok_and(|m| m.is_match(rel))
        })
        .collect()
}
