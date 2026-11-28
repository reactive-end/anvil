//! Refreshing a concept after its code was reviewed, and recording human verification.

use std::path::Path;

use anvil_core::fs_util::atomic_write;
use anvil_graph::model::CodeGraph;
use anyhow::{Result, bail};

use crate::bundle::Bundle;
use crate::concept::Concept;
use crate::create::append_log;
use crate::meta::Stamp;
use crate::paths::PathMatcher;
use crate::{KEY_SYNCED_COMMIT, KEY_SYNCED_DIGEST, drift, now_rfc3339, outline, producer};

/// What `refresh_concept` changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refreshed {
    pub key: String,
    /// `true` when the outline block was regenerated.
    pub outline_updated: bool,
    /// Commit the concept is now synced to.
    pub synced_commit: Option<String>,
}

/// Regenerates the outline block (when the body has one and a graph is available), stamps `generated`
/// and the sync state, and logs the refresh. Everything outside the outline block stays byte for byte.
pub fn refresh_concept(
    root: &Path,
    bundle: &Bundle,
    concept: &Concept,
    graph: Option<&CodeGraph>,
) -> Result<Refreshed> {
    let mut doc = concept.doc.clone();
    let globs = doc.code_paths();
    let mut outline_updated = false;
    if let Some(graph) = graph {
        let inner = outline::render(graph, &PathMatcher::new(&globs)?);
        if let Some(body) = outline::replace(doc.body(), &inner) {
            doc.set_body(body);
            outline_updated = true;
        }
    }
    doc.set_generated(&Stamp { by: producer(), at: now_rfc3339() });
    let (commit, digest) = drift::sync_state(root, &globs);
    set_or_remove(&mut doc, KEY_SYNCED_COMMIT, commit.as_deref());
    set_or_remove(&mut doc, KEY_SYNCED_DIGEST, digest.as_deref());
    atomic_write(&concept.path, doc.render().as_bytes())?;
    let key = concept.key();
    append_log(bundle, &format!("Refreshed [{key}]({}.md).", concept.id))?;
    Ok(Refreshed { key, outline_updated, synced_commit: commit })
}

fn set_or_remove(doc: &mut crate::frontmatter::Document, key: &str, value: Option<&str>) {
    match value {
        Some(value) => doc.set_str(key, value),
        None => doc.remove_field(key),
    }
}

/// Appends a `human:<name>` verification stamped now. Only a person may call this.
pub fn verify_concept(bundle: &Bundle, concept: &Concept, name: &str) -> Result<Stamp> {
    let name = name.trim();
    if name.is_empty() {
        bail!("verifier name is empty (pass --by <name> or set `git config user.name`)");
    }
    let stamp = Stamp { by: format!("human:{name}"), at: now_rfc3339() };
    let mut doc = concept.doc.clone();
    doc.push_verified(&stamp);
    atomic_write(&concept.path, doc.render().as_bytes())?;
    append_log(
        bundle,
        &format!("Verified [{}]({}.md) by {}.", concept.key(), concept.id, stamp.by),
    )?;
    Ok(stamp)
}
