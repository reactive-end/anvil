//! Word search over non-test sources and the `anvil-allow(overengineering)` escape hatch.

use std::cell::OnceCell;
use std::fs;
use std::path::Path;

use anvil_core::traits::Language;

use super::conventions::{is_ignored_path, is_test_like};
use crate::model::CodeGraph;

const ALLOW_MARKER: &str = "anvil-allow(overengineering)";
const COMMENT_TOKENS: [&str; 5] = ["//", "#", "/*", "--", "<!--"];
const MAX_SOURCE_BYTES: u64 = 1_000_000;

/// Contents of the non-test code files in the graph, read on first use.
#[derive(Default)]
pub(super) struct Corpus {
    files: OnceCell<Vec<(String, String)>>,
}

impl Corpus {
    /// `true` when `name` appears as a whole word in a non-test file other than `owner`.
    pub(super) fn mentioned_outside(&self, graph: &CodeGraph, owner: &str, name: &str) -> bool {
        let files = self.files.get_or_init(|| load(graph));
        files.iter().any(|(file, source)| file != owner && count_word(source, name) > 0)
    }
}

fn load(graph: &CodeGraph) -> Vec<(String, String)> {
    let root = Path::new(&graph.repo_root);
    let mut paths: Vec<&String> =
        graph.file_hashes.keys().filter(|p| is_code_file(graph, p)).collect();
    paths.sort();
    paths
        .into_iter()
        .filter(|p| !is_test_like(p) && !is_ignored_path(p))
        .filter_map(|p| read_small(&root.join(p)).map(|source| (p.clone(), source)))
        .collect()
}

fn is_code_file(graph: &CodeGraph, path: &str) -> bool {
    let language = graph
        .file_nodes
        .get(path)
        .into_iter()
        .flatten()
        .filter_map(|&nid| graph.get_node(nid))
        .find_map(|n| n.language);
    !matches!(
        language,
        None | Some(Language::Json | Language::Yaml | Language::Markdown | Language::Other)
    )
}

fn read_small(path: &Path) -> Option<String> {
    let small = fs::metadata(path).ok()?.len() <= MAX_SOURCE_BYTES;
    small.then(|| fs::read_to_string(path).ok()).flatten()
}

/// Counts whole-word occurrences of `name` (identifier characters on either side do not match).
pub(super) fn count_word(source: &str, name: &str) -> usize {
    if name.is_empty() {
        return 0;
    }
    let bytes = source.as_bytes();
    let mut count = 0;
    let mut from = 0;
    while let Some(pos) = source[from..].find(name) {
        let start = from + pos;
        let end = start + name.len();
        let before_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_ident_byte(bytes[end]);
        count += usize::from(before_ok && after_ok);
        from = end;
    }
    count
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte >= 0x80
}

/// `anvil-allow(overengineering): <reason>` in the leading comment block of the file.
pub(super) fn allows_file(source: &str) -> bool {
    source
        .lines()
        .map(str::trim)
        .take_while(|line| line.is_empty() || is_comment_or_attribute(line))
        .any(has_allow_marker)
}

/// The marker in the comment or attribute lines directly above the symbol at 1-based `line`.
pub(super) fn allows_symbol(source: &str, line: u32) -> bool {
    let lines: Vec<&str> = source.lines().collect();
    let def_index = (line as usize).saturating_sub(1).min(lines.len());
    lines[..def_index]
        .iter()
        .rev()
        .map(|l| l.trim())
        .take_while(|l| !l.is_empty() && is_comment_or_attribute(l))
        .any(has_allow_marker)
}

fn is_comment_or_attribute(line: &str) -> bool {
    COMMENT_TOKENS.iter().any(|t| line.starts_with(t))
        || line.starts_with('*')
        || line.starts_with('@')
}

fn has_allow_marker(line: &str) -> bool {
    let Some(at) = line.find(ALLOW_MARKER) else {
        return false;
    };
    let in_comment = COMMENT_TOKENS.iter().any(|t| line[..at].contains(t)) || line.starts_with('*');
    let reason = line[at + ALLOW_MARKER.len()..].trim_start().strip_prefix(':');
    in_comment && reason.is_some_and(|r| r.chars().any(char::is_alphanumeric))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_match_whole_identifiers_only() {
        assert_eq!(count_word("fn run() { run(); runner(); _run(); }", "run"), 2);
        assert_eq!(count_word("héllo(x) hello", "hello"), 1);
    }

    #[test]
    fn allow_marker_needs_a_comment_and_a_reason() {
        assert!(has_allow_marker("// anvil-allow(overengineering): public API of the crate"));
        assert!(has_allow_marker("# anvil-allow(overengineering): used by the CLI shim"));
        assert!(!has_allow_marker("// anvil-allow(overengineering):"));
        assert!(!has_allow_marker("let s = \"anvil-allow(overengineering): x\";"));
        assert!(!has_allow_marker("// anvil-allow(other): reason"));
    }

    #[test]
    fn allow_applies_to_the_file_header_or_the_line_above() {
        let source =
            "// anvil-allow(overengineering): kept for the plugin API\nexport const a = 1;\n";
        assert!(allows_file(source));
        let source = "export const a = 1;\n// anvil-allow(overengineering): kept\n#[x]\nexport const b = 2;\n";
        assert!(!allows_file(source));
        assert!(allows_symbol(source, 4));
        assert!(!allows_symbol(source, 1));
    }
}
