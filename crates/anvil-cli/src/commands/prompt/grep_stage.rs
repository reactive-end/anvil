//! The CLI-only lookup that finds symbols by what their body says. The graph indexes names, not
//! fields or string contents, so "quantity" never finds the `Order` class that has a `quantity`
//! field; a text search plus the enclosing symbol of each match does. Hooks never run this.

use std::cmp::Reverse;

use anvil_core::diff_grep::{GrepOptions, GrepReport, grep_with_backend};
use anvil_graph::model::{CodeGraph, GraphNode, NodeKind};

use super::gather::{MAX_CONTENT_CODE, Run, is_noise_path, push_code};
use super::refs::{CodeRef, Gathered};
use super::stem::stem;

/// Words searched, longest first: short ones match everywhere.
const MAX_WORDS: usize = 3;
/// Shorter words are not worth a text search.
const MIN_WORD_CHARS: usize = 5;
/// A stem shorter than this would match everywhere, so the whole word is searched instead.
const MIN_STEM_CHARS: usize = 4;
/// The stage is skipped once the graph already found this many items.
const ENOUGH: usize = 3;
const MAX_MATCHES: usize = 80;
const MAX_PER_FILE: usize = 8;
const MAX_TOKENS: usize = 1500;
/// Files that mention words without being code to start from.
const PROSE_EXTENSIONS: &[&str] = &[".md", ".json", ".lock", ".txt"];

/// Adds the symbols that mention the prompt's words, most mentions first.
pub fn content_grep(run: &Run<'_>, out: &mut Gathered) {
    let Some(graph) = &run.graph else {
        return;
    };
    if !run.limits.grep || out.code.len() >= ENOUGH {
        return;
    }
    let words = search_words(&run.terms.words);
    let mut tally: Vec<(CodeRef, usize)> = Vec::new();
    for word in words.iter().take(MAX_WORDS) {
        if run.out_of_time() {
            break;
        }
        match search(run, word) {
            Ok(report) => count_symbols(graph, &report, &mut tally),
            Err(err) => out.problems.push(format!("text search for `{word}` failed: {err:#}")),
        }
    }
    tally.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.file.cmp(&b.0.file)));
    let room = run.limits.code.min(out.code.len() + MAX_CONTENT_CODE);
    for (found, _) in tally {
        push_code(out, found, room);
    }
}

/// What to search for: the stem of each long enough word (a substring, so it also finds the other
/// inflections of the word), longest first, once each.
fn search_words(words: &[String]) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in words.iter().filter(|w| w.chars().count() >= MIN_WORD_CHARS) {
        let stemmed = stem(word);
        let term = if stemmed.chars().count() >= MIN_STEM_CHARS { stemmed } else { word.clone() };
        if !terms.contains(&term) {
            terms.push(term);
        }
    }
    terms.sort_by_key(|t| Reverse(t.chars().count()));
    terms
}

fn search(run: &Run<'_>, word: &str) -> anyhow::Result<GrepReport> {
    let mut opts = GrepOptions::new(word);
    opts.fixed_strings = true;
    opts.max_matches = MAX_MATCHES;
    opts.max_per_file = MAX_PER_FILE;
    opts.max_tokens = MAX_TOKENS;
    grep_with_backend(run.root, &opts, run.config.search.backend)
}

fn count_symbols(graph: &CodeGraph, report: &GrepReport, tally: &mut Vec<(CodeRef, usize)>) {
    for file in &report.files {
        let skipped = is_noise_path(&file.path)
            || PROSE_EXTENSIONS.iter().any(|ext| file.path.ends_with(ext));
        if skipped {
            continue;
        }
        for hit in &file.matches {
            let Some(node) = outermost_symbol(graph, &file.path, hit.line) else {
                continue;
            };
            let found = code_ref(node);
            match tally.iter_mut().find(|(known, _)| known.key() == found.key()) {
                Some((_, count)) => *count += 1,
                None => tally.push((found, 1)),
            }
        }
    }
}

/// The widest symbol of `file` that contains `line`: the class rather than its method, since the
/// class is where a reader should start.
fn outermost_symbol<'g>(graph: &'g CodeGraph, file: &str, line: u32) -> Option<&'g GraphNode> {
    let norm = CodeGraph::normalize_rel_path(file);
    graph
        .file_nodes
        .get(&norm)?
        .iter()
        .filter_map(|&id| graph.get_node(id))
        .filter(|n| n.kind == NodeKind::Symbol && n.line <= line && line <= n.end_line.max(n.line))
        .max_by_key(|n| n.end_line.max(n.line) - n.line)
}

fn code_ref(node: &GraphNode) -> CodeRef {
    let kind = node.symbol_kind.map_or("symbol".to_owned(), |k| format!("{k:?}").to_lowercase());
    CodeRef { name: node.label.clone(), kind, file: node.file.clone(), line: Some(node.line) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_the_stem_so_other_inflections_match() {
        let words = vec!["cancelled".to_owned(), "stock".to_owned(), "orders".to_owned()];
        assert_eq!(search_words(&words), ["cancel", "stock", "order"]);
    }

    #[test]
    fn a_short_stem_falls_back_to_the_whole_word() {
        assert_eq!(search_words(&["creaciones".to_owned()]), ["creaciones"]);
    }
}
