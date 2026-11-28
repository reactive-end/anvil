//! Markdown link extraction and resolution against a bundle.

use std::path::{Path, PathBuf};

/// Where a link points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A URL, an anchor-only link, or a path that leaves the bundle: never read from disk.
    External,
    /// A path inside the bundle, relative to the bundle root.
    Inside(PathBuf),
}

/// Link destinations in `body`, skipping fenced code blocks and inline code spans.
pub fn extract(body: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut fence: Option<&str> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        let marker = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m));
        match (fence, marker) {
            (Some(open), Some(m)) if open == m => fence = None,
            (None, Some(m)) => fence = Some(m),
            (None, None) => collect_line(&strip_code_spans(line), &mut targets),
            _ => {}
        }
    }
    targets
}

fn strip_code_spans(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    for ch in line.chars() {
        if ch == '`' {
            in_code = !in_code;
        } else if !in_code {
            out.push(ch);
        }
    }
    out
}

fn collect_line(line: &str, targets: &mut Vec<String>) {
    let mut rest = line;
    while let Some(pos) = rest.find("](") {
        let after = &rest[pos + 2..];
        let (target, consumed) = take_destination(after);
        if !target.is_empty() {
            targets.push(target);
        }
        rest = &after[consumed..];
    }
    if let Some(def) = reference_definition(line) {
        targets.push(def);
    }
}

/// Reads one inline-link destination up to its closing parenthesis; returns it and bytes consumed.
fn take_destination(text: &str) -> (String, usize) {
    let mut depth = 0usize;
    let mut end = text.len();
    for (idx, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 0 => {
                end = idx;
                break;
            }
            ')' => depth -= 1,
            _ => {}
        }
    }
    let inner = text[..end].trim();
    let destination = match inner.strip_prefix('<') {
        Some(angled) => angled.split('>').next().unwrap_or_default(),
        None => inner.split_whitespace().next().unwrap_or_default(),
    };
    (destination.to_owned(), (end + 1).min(text.len()))
}

/// `[label]: destination` link reference definitions.
fn reference_definition(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix('[')?;
    let (_, after) = rest.split_once("]:")?;
    let target = after.split_whitespace().next()?;
    Some(target.trim_matches(['<', '>']).to_owned())
}

fn has_scheme(target: &str) -> bool {
    let Some((scheme, _)) = target.split_once(':') else {
        return false;
    };
    scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// Resolves `target` found in the file at `file_rel` (bundle-relative path of the Markdown file).
/// A leading `/` anchors at the bundle root; anything that leaves the bundle is external.
pub fn resolve(file_rel: &Path, target: &str) -> Target {
    let bare = target.split(['#', '?']).next().unwrap_or_default().replace("%20", " ");
    if bare.is_empty() || has_scheme(&bare) || bare.starts_with("//") {
        return Target::External;
    }
    let mut stack: Vec<String> = Vec::new();
    let relative = match bare.strip_prefix('/') {
        Some(from_root) => from_root.to_owned(),
        None => {
            let parent = file_rel.parent().unwrap_or_else(|| Path::new(""));
            stack.extend(parent.components().map(|c| c.as_os_str().to_string_lossy().into_owned()));
            bare
        }
    };
    for part in relative.replace('\\', "/").split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if stack.pop().is_none() {
                    return Target::External;
                }
            }
            other => stack.push(other.to_owned()),
        }
    }
    Target::Inside(stack.iter().collect())
}

/// `true` when an in-bundle target exists as a file or directory under `bundle_dir`.
pub fn exists(bundle_dir: &Path, inside: &Path) -> bool {
    bundle_dir.join(inside).exists()
}
