//! YAML frontmatter parsing and rendering that keeps unknown keys, key order, and the body intact.

use std::fmt;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use crate::yaml::{self, Mapping, MappingError};

/// Why a file's frontmatter could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontmatterError {
    /// The opening `---` has no closing delimiter.
    Unterminated,
    /// The block between the delimiters is not valid YAML.
    Yaml(String),
    /// The block is valid YAML but not a mapping.
    NotMapping,
}

impl fmt::Display for FrontmatterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unterminated => f.write_str("frontmatter is not closed by a `---` line"),
            Self::Yaml(err) => write!(f, "frontmatter is not valid YAML: {err}"),
            Self::NotMapping => f.write_str("frontmatter must be a YAML mapping"),
        }
    }
}

impl std::error::Error for FrontmatterError {}

/// A Markdown file split into its frontmatter mapping and body.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    meta: Option<Mapping>,
    /// Original text up to and including the closing delimiter line (empty without frontmatter).
    head: String,
    body: String,
    dirty: bool,
}

/// Text pieces of a file with frontmatter.
struct Split<'a> {
    head: &'a str,
    yaml: &'a str,
    body: &'a str,
}

fn is_delimiter(line: &str) -> bool {
    line.trim_end() == "---"
}

fn split(text: &str) -> Result<Option<Split<'_>>, FrontmatterError> {
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Ok(None);
    };
    if !is_delimiter(first) {
        return Ok(None);
    }
    let yaml_start = first.len();
    let mut offset = yaml_start;
    for line in lines {
        if is_delimiter(line) || line.trim_end() == "..." {
            let end = offset + line.len();
            return Ok(Some(Split {
                head: &text[..end],
                yaml: &text[yaml_start..offset],
                body: &text[end..],
            }));
        }
        offset += line.len();
    }
    Err(FrontmatterError::Unterminated)
}

impl Document {
    /// Parses `text`; a file without a leading `---` block has no metadata and keeps all text as body.
    pub fn parse(text: &str) -> Result<Self, FrontmatterError> {
        let bom = if text.starts_with('\u{feff}') { "\u{feff}" } else { "" };
        let Some(parts) = split(&text[bom.len()..])? else {
            return Ok(Self {
                meta: None,
                head: String::new(),
                body: text.to_owned(),
                dirty: false,
            });
        };
        let meta = yaml::parse_mapping(parts.yaml).map_err(|err| match err {
            MappingError::Syntax(reason) => FrontmatterError::Yaml(reason),
            MappingError::NotMapping => FrontmatterError::NotMapping,
        })?;
        Ok(Self {
            meta: Some(meta),
            head: format!("{bom}{}", parts.head),
            body: parts.body.to_owned(),
            dirty: false,
        })
    }

    /// Builds a document from scratch.
    pub fn new(meta: Mapping, body: impl Into<String>) -> Self {
        Self { meta: Some(meta), head: String::new(), body: body.into(), dirty: true }
    }

    /// Frontmatter mapping, or `None` when the file has no frontmatter block.
    pub fn meta(&self) -> Option<&Mapping> {
        self.meta.as_ref()
    }

    /// Mutable frontmatter mapping (created when absent); marks the head as needing re-serialisation.
    pub fn meta_mut(&mut self) -> &mut Mapping {
        self.dirty = true;
        self.meta.get_or_insert_with(Mapping::new)
    }

    /// Markdown after the frontmatter, byte for byte.
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Replaces the body.
    pub fn set_body(&mut self, body: String) {
        self.body = body;
    }

    /// Serialises the document. An untouched frontmatter block is written back exactly as read.
    pub fn render(&self) -> String {
        let Some(meta) = self.meta.as_ref() else {
            return self.body.clone();
        };
        if !self.dirty {
            return format!("{}{}", self.head, self.body);
        }
        let yaml = yaml::emit(meta);
        format!("---\n{yaml}---\n{}", self.body)
    }
}

/// Reads only the frontmatter block of `path` (stops at the closing delimiter), leaving the body empty.
pub fn read_head(path: &Path) -> io::Result<String> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut head = String::new();
    let mut line = String::new();
    let mut first = true;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(head);
        }
        let text = if first { line.trim_start_matches('\u{feff}') } else { line.as_str() };
        if first && !is_delimiter(text) {
            return Ok(String::new());
        }
        head.push_str(text);
        if !first && (is_delimiter(text) || text.trim_end() == "...") {
            return Ok(head);
        }
        first = false;
    }
}
