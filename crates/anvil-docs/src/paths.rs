//! Glob matching for `x-anvil-paths`.

use anyhow::{Context, Result};
use globset::{Glob, GlobBuilder, GlobMatcher};

/// One declared glob, compiled.
#[derive(Debug, Clone)]
struct Pattern {
    source: String,
    matchers: Vec<GlobMatcher>,
}

impl Pattern {
    fn compile(source: &str) -> Result<Self> {
        let trimmed = source.trim().trim_start_matches("./").replace('\\', "/");
        let mut globs = vec![trimmed.clone()];
        // A plain directory path documents everything below it.
        if !trimmed.contains(['*', '?', '[', '{']) {
            globs.push(format!("{}/**", trimmed.trim_end_matches('/')));
        }
        let matchers = globs.iter().map(|g| build(g)).collect::<Result<Vec<_>>>()?;
        Ok(Self { source: source.to_owned(), matchers })
    }

    fn is_match(&self, path: &str) -> bool {
        self.matchers.iter().any(|m| m.is_match(path))
    }
}

fn build(glob: &str) -> Result<GlobMatcher> {
    let compiled: Glob = GlobBuilder::new(glob)
        .literal_separator(true)
        .build()
        .with_context(|| format!("invalid glob `{glob}`"))?;
    Ok(compiled.compile_matcher())
}

/// The globs a concept documents; matches repo-relative, forward-slash paths.
#[derive(Debug, Clone, Default)]
pub struct PathMatcher {
    patterns: Vec<Pattern>,
}

impl PathMatcher {
    /// Compiles `globs`, failing on the first invalid one.
    pub fn new(globs: &[String]) -> Result<Self> {
        let patterns = globs.iter().map(|g| Pattern::compile(g)).collect::<Result<Vec<_>>>()?;
        Ok(Self { patterns })
    }

    /// `true` when no glob is declared.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// `true` when any glob matches `path`.
    pub fn is_match(&self, path: &str) -> bool {
        self.patterns.iter().any(|p| p.is_match(path))
    }

    /// Globs that match none of `files`.
    pub fn unmatched<'a>(&'a self, files: &[String]) -> Vec<&'a str> {
        self.patterns
            .iter()
            .filter(|p| !files.iter().any(|f| p.is_match(f)))
            .map(|p| p.source.as_str())
            .collect()
    }
}
