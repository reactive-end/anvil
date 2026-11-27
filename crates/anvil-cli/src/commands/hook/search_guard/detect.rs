//! Classifies a tool call as a code search, a consultation of the graph or docs, or neither.

mod shell;

use std::path::{Path, PathBuf};

use anvil_adapters::contract::NormalizedHookInput;
use serde_json::{Map, Value};

use super::super::payload::{
    extract_command_field, extract_rel_file, is_within_root, strip_tool_namespace,
};

const GREP_TOOLS: &[&str] = &["Grep", "grep_search"];
const GLOB_TOOLS: &[&str] = &["Glob", "find_by_name"];
const READ_TOOLS: &[&str] =
    &["Read", "read_file", "view_file", "view_file_outline", "view_code_item"];
const PATTERN_KEYS: &[&str] = &["pattern", "Pattern", "Query", "query"];
const PATH_KEYS: &[&str] = &["path", "SearchPath", "SearchDirectory", "paths"];
const DOCS_DIR: &str = ".harness/docs";

/// What a tool call is for the lookup cascade.
#[derive(Debug, PartialEq, Eq)]
pub enum Call {
    /// A search of the repository's code.
    Search(SearchCall),
    /// A lookup in the graph or docs.
    Consult,
    Other,
}

/// A code search reduced to the words the graph and docs can be asked about.
#[derive(Debug, PartialEq, Eq)]
pub struct SearchCall {
    pub query: String,
}

/// Classifies `input`; anything that cannot be parsed is `Other`.
pub fn classify(root: &Path, input: &NormalizedHookInput) -> Call {
    let Some(name) = input.tool_name.as_deref() else {
        return Call::Other;
    };
    let tool_input = input.tool_input.as_ref();
    if let Some((server, tool)) = mcp_parts(name) {
        return if server.contains("anvil") {
            classify_anvil_mcp(root, tool, tool_input)
        } else {
            Call::Other
        };
    }
    let bare = strip_tool_namespace(Some(name)).unwrap_or_default();
    if GREP_TOOLS.contains(&bare) {
        return tool_search(root, tool_input, query_of_pattern);
    }
    if GLOB_TOOLS.contains(&bare) {
        return tool_search(root, tool_input, query_of_glob);
    }
    if READ_TOOLS.contains(&bare) {
        return read_call(root, tool_input);
    }
    match extract_command_field(tool_input) {
        Some((_, cmd)) => shell::classify(root, cmd),
        None => Call::Other,
    }
}

/// Splits `mcp__<server>__<tool>`.
fn mcp_parts(name: &str) -> Option<(&str, &str)> {
    name.strip_prefix("mcp__")?.rsplit_once("__")
}

fn classify_anvil_mcp(root: &Path, tool: &str, tool_input: Option<&Value>) -> Call {
    match tool {
        "search" | "context_pack" | "code_exec" => Call::Consult,
        _ if tool.starts_with("graph_") => Call::Consult,
        "grep" => tool_search(root, tool_input, query_of_pattern),
        _ => Call::Other,
    }
}

/// A read of a concept under `.harness/docs/` consults the docs.
fn read_call(root: &Path, tool_input: Option<&Value>) -> Call {
    match extract_rel_file(root, tool_input) {
        Some(rel) if is_docs_path(&rel) => Call::Consult,
        _ => Call::Other,
    }
}

/// Builds the call from the pattern and path fields of a search tool.
fn tool_search(root: &Path, tool_input: Option<&Value>, to_query: fn(&str) -> String) -> Call {
    let Some(obj) = tool_input.and_then(Value::as_object) else {
        return Call::Other;
    };
    let Some(pattern) = first_str(obj, PATTERN_KEYS) else {
        return Call::Other;
    };
    search_call(root, &to_query(pattern), &paths_of(obj))
}

/// Shared tail of every detector: the query must be searchable and the search must reach the repo.
pub(super) fn search_call(root: &Path, query: &str, paths: &[String]) -> Call {
    if !query.chars().any(char::is_alphanumeric) {
        return Call::Other;
    }
    let inside: Vec<&String> = paths.iter().filter(|p| in_repo(root, p)).collect();
    if !paths.is_empty() && inside.is_empty() {
        return Call::Other;
    }
    if !inside.is_empty() && inside.iter().all(|p| is_docs_path(p)) {
        return Call::Consult;
    }
    Call::Search(SearchCall { query: query.to_owned() })
}

fn first_str<'a>(obj: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// `path` as a string or `paths` as an array of strings.
fn paths_of(obj: &Map<String, Value>) -> Vec<String> {
    let mut out = Vec::new();
    for key in PATH_KEYS {
        match obj.get(*key) {
            Some(Value::String(one)) if !one.trim().is_empty() => out.push(one.trim().to_owned()),
            Some(Value::Array(many)) => {
                out.extend(many.iter().filter_map(Value::as_str).map(|s| s.trim().to_owned()));
            }
            _ => {}
        }
    }
    out
}

/// Words of a regex worth asking the graph about: escapes such as `\s` or `\b` are dropped.
fn query_of_pattern(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            chars.next();
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

/// Words of a glob: the stems of its file names (`**/search_guard*.rs` -> `search_guard`).
fn query_of_glob(pattern: &str) -> String {
    let spaced: String = pattern
        .chars()
        .map(|c| {
            if matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | '/' | '\\' | ',' | '!') {
                ' '
            } else {
                c
            }
        })
        .collect();
    spaced
        .split_whitespace()
        .filter_map(|word| word.split('.').next())
        .filter(|stem| stem.chars().count() >= 3)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether the path reaches into the repository (relative paths always do unless they climb out).
pub(super) fn in_repo(root: &Path, raw: &str) -> bool {
    let cleaned = raw.trim().trim_matches(['"', '\'']);
    if cleaned.is_empty() {
        return true;
    }
    if cleaned.starts_with('~') {
        return false;
    }
    let path = Path::new(cleaned);
    let abs: PathBuf = if path.is_absolute() || cleaned.starts_with(['/', '\\']) {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    is_within_root(root, &abs)
}

/// Whether the (repo-relative or absolute) path is inside `.harness/docs/`.
pub(super) fn is_docs_path(raw: &str) -> bool {
    let norm = raw.replace('\\', "/");
    let norm = norm.trim_start_matches("./");
    norm == DOCS_DIR
        || norm.starts_with(".harness/docs/")
        || norm.contains("/.harness/docs/")
        || norm.trim_end_matches('/').ends_with("/.harness/docs")
}
