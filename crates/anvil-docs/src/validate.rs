//! OKF validation of bundles and single files.

use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::bundle::Bundle;
use crate::concept::{is_reserved, markdown_files};
use crate::frontmatter::Document;
use crate::links::{self, Target};
use crate::meta::STATUSES;
use crate::paths::PathMatcher;
use crate::yaml::Yaml;
use crate::{KEY_PATHS, OKF_VERSION};

/// How serious a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// One validation finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    /// Repo-relative file the finding is about.
    pub file: String,
    pub severity: Severity,
    pub message: String,
}

struct Ctx<'a> {
    root: &'a Path,
    bundle: &'a Bundle,
    repo_files: &'a [String],
}

/// Validates every Markdown file of `bundle`. `repo_files` is the repository file list used to check
/// that each `x-anvil-paths` glob matches something.
pub fn validate_bundle(root: &Path, bundle: &Bundle, repo_files: &[String]) -> Vec<Issue> {
    let mut issues = Vec::new();
    if !bundle.dir.is_dir() {
        return issues;
    }
    if !bundle.index_path().is_file() {
        issues.push(Issue {
            file: format!("{}/index.md", bundle.repo_rel()),
            severity: Severity::Error,
            message: "bundle root has no index.md".to_owned(),
        });
    }
    for path in markdown_files(&bundle.dir) {
        issues.extend(validate_file(root, bundle, &path, repo_files));
    }
    issues
}

/// Validates one Markdown file of `bundle`.
pub fn validate_file(
    root: &Path,
    bundle: &Bundle,
    path: &Path,
    repo_files: &[String],
) -> Vec<Issue> {
    let ctx = Ctx { root, bundle, repo_files };
    let mut found = Findings::new(&ctx, path);
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            found.error(format!("cannot read file: {err}"));
            return found.issues;
        }
    };
    let doc = match Document::parse(&text) {
        Ok(doc) => doc,
        Err(err) => {
            found.error(err.to_string());
            return found.issues;
        }
    };
    if is_reserved(path) {
        check_reserved(&mut found, &doc, path);
    } else if doc.meta().is_some() {
        check_concept(&mut found, &doc);
    } else {
        return found.issues;
    }
    check_links(&mut found, &doc, path);
    found.issues
}

struct Findings<'a> {
    ctx: &'a Ctx<'a>,
    file: String,
    issues: Vec<Issue>,
}

impl<'a> Findings<'a> {
    fn new(ctx: &'a Ctx<'a>, path: &Path) -> Self {
        let file = path.strip_prefix(ctx.root).unwrap_or(path).to_string_lossy().replace('\\', "/");
        Self { ctx, file, issues: Vec::new() }
    }

    fn push(&mut self, severity: Severity, message: String) {
        self.issues.push(Issue { file: self.file.clone(), severity, message });
    }

    fn error(&mut self, message: String) {
        self.push(Severity::Error, message);
    }
}

fn check_reserved(found: &mut Findings<'_>, doc: &Document, path: &Path) {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if doc.concept_type().is_some() {
        found.error(format!("`{name}` is reserved and cannot be a concept (remove `type`)"));
    }
    let is_root_index = name == "index.md" && path.parent() == Some(found.ctx.bundle.dir.as_path());
    if is_root_index {
        match doc.str_field("okf_version") {
            None => {
                found.error(format!("root index.md must declare `okf_version: \"{OKF_VERSION}\"`"))
            }
            Some(version) if version != OKF_VERSION => found.push(
                Severity::Warning,
                format!("root index.md declares okf_version `{version}`, expected `{OKF_VERSION}`"),
            ),
            Some(_) => {}
        }
    }
}

fn check_concept(found: &mut Findings<'_>, doc: &Document) {
    match doc.field("type") {
        None => found.error("missing required `type`".to_owned()),
        Some(Yaml::String(t)) if !t.trim().is_empty() => {}
        Some(_) => found.error("`type` must be a non-empty string".to_owned()),
    }
    for key in ["title", "description"] {
        if doc.field(key).is_some_and(|v| !matches!(v, Yaml::String(_))) {
            found.error(format!("`{key}` must be a string"));
        }
    }
    if let Some(status) = doc.status()
        && !STATUSES.contains(&status)
    {
        found.error(format!("`status` must be one of {}, got `{status}`", STATUSES.join("|")));
    }
    check_stamps(found, doc);
    check_code_paths(found, doc);
}

fn check_stamps(found: &mut Findings<'_>, doc: &Document) {
    if doc.field("generated").is_some() && doc.generated().is_none_or(|g| g.instant().is_none()) {
        found.error("`generated` needs `by` and an RFC 3339 `at`".to_owned());
    }
    if let Some(value) = doc.field("verified") {
        let entries = value.as_vec().map_or(0, Vec::len);
        let valid = doc.verified().iter().filter(|v| v.instant().is_some()).count();
        if entries != valid {
            found.error("every `verified` entry needs `by` and an RFC 3339 `at`".to_owned());
        }
    }
    if doc.field("stale_after").is_some() && doc.stale_after().is_none() {
        found.error("`stale_after` must be an RFC 3339 timestamp".to_owned());
    }
}

fn check_code_paths(found: &mut Findings<'_>, doc: &Document) {
    let Some(raw) = doc.field(KEY_PATHS) else {
        return;
    };
    let is_list =
        raw.as_vec().is_some_and(|items| items.iter().all(|i| matches!(i, Yaml::String(_))));
    if !is_list {
        found.error(format!("`{KEY_PATHS}` must be a list of glob strings"));
        return;
    }
    match PathMatcher::new(&doc.code_paths()) {
        Err(err) => found.error(format!("`{KEY_PATHS}`: {err:#}")),
        Ok(matcher) => {
            for glob in matcher.unmatched(found.ctx.repo_files) {
                found.error(format!("`{KEY_PATHS}` glob `{glob}` matches no file"));
            }
        }
    }
}

fn check_links(found: &mut Findings<'_>, doc: &Document, path: &Path) {
    let bundle_dir = &found.ctx.bundle.dir;
    let file_rel = path.strip_prefix(bundle_dir).unwrap_or(path);
    for target in links::extract(doc.body()) {
        if let Target::Inside(inside) = links::resolve(file_rel, &target)
            && !links::exists(bundle_dir, &inside)
        {
            found.error(format!("broken link `{target}`"));
        }
    }
}
