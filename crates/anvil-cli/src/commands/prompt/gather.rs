//! Looks the terms of a prompt up in the code graph, the living docs, and the memories, and keeps
//! only strong matches: an identifier that resolves, or a hit that shares at least two distinct
//! words with the prompt. Nothing relevant means nothing is returned.

use std::path::Path;
use std::time::{Duration, Instant};

use anvil_core::config::AnvilConfig;
use anvil_core::traits::FreshnessState;
use anvil_docs::concept::{self, Concept};
use anvil_docs::drift::Drift;
use anvil_docs::{bundle, search as docs_search};
use anvil_graph::model::{CodeGraph, NodeKind};
use anvil_graph::{query, store};
use anvil_memory::MemoryEngine;
use anvil_tasks::resolve_author_fast;

use super::grep_stage;
use super::refs::{CodeRef, DocRef, Gathered, MemoryRef};
use super::stopwords::fold_accents;
use super::terms::{Ident, IdentKind, Terms, has_file_extension, tokens};

/// Hits examined per content search before the strength filter.
const CANDIDATES: usize = 8;
/// A single shared word only counts when it is at least this long and names the hit itself.
const SINGLE_WORD_MIN_CHARS: usize = 5;
/// Symbols resolved from one identifier (an identifier can name a class and its constructor).
const PER_IDENT: usize = 2;
/// Items the content search may add on top of the resolved identifiers.
pub(super) const MAX_CONTENT_CODE: usize = 3;

/// How many items of each kind to keep, and how long gathering may take.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub code: usize,
    pub docs: usize,
    pub memories: usize,
    pub time: Duration,
    /// Search the source text for the prompt's words, to find the symbols that mention them. Only
    /// for the CLI: a hook never runs a text search.
    pub grep: bool,
}

/// The loaded graph and the clock, shared by every stage.
pub(super) struct Run<'a> {
    pub root: &'a Path,
    pub config: &'a AnvilConfig,
    pub terms: &'a Terms,
    pub limits: Limits,
    pub started: Instant,
    pub graph: Option<CodeGraph>,
}

impl Run<'_> {
    pub fn out_of_time(&self) -> bool {
        self.started.elapsed() >= self.limits.time
    }
}

/// Looks `terms` up. Stops starting new stages once `limits.time` has passed.
pub fn gather(root: &Path, config: &AnvilConfig, terms: &Terms, limits: Limits) -> Gathered {
    let mut out = Gathered {
        names_a_file: terms
            .idents
            .iter()
            .any(|i| i.kind == IdentKind::Path && has_file_extension(&i.text)),
        ..Gathered::default()
    };
    let mut run = Run { root, config, terms, limits, started: Instant::now(), graph: None };
    run.graph = load_graph(root, &mut out.problems);
    resolve_idents(&run, &mut out);
    let stages: [fn(&Run<'_>, &mut Gathered); 4] =
        [content_code, grep_stage::content_grep, content_docs, content_memories];
    for stage in stages {
        if run.out_of_time() {
            out.out_of_time = true;
            break;
        }
        stage(&run, &mut out);
    }
    out.elapsed_ms = u64::try_from(run.started.elapsed().as_millis()).unwrap_or(u64::MAX);
    out
}

/// Loads the cached graph without ever building one (a hook must not). Unreadable means `None`
/// plus a note in `problems`.
pub fn load_graph(root: &Path, problems: &mut Vec<String>) -> Option<CodeGraph> {
    let bin = store::default_bin_path(root);
    if !bin.is_file() {
        return None;
    }
    match store::load_binary(&bin) {
        Ok(graph) => Some(graph),
        Err(err) => {
            problems.push(format!("graph unreadable: {err:#}"));
            None
        }
    }
}

fn resolve_idents(run: &Run<'_>, out: &mut Gathered) {
    let Some(graph) = &run.graph else {
        return;
    };
    for ident in &run.terms.idents {
        for found in resolve_ident(graph, ident) {
            push_code(out, found, run.limits.code);
        }
    }
}

pub(super) fn push_code(out: &mut Gathered, found: CodeRef, cap: usize) {
    if out.code.len() < cap && !out.code.iter().any(|c| c.key() == found.key()) {
        out.code.push(found);
    }
}

fn resolve_ident(graph: &CodeGraph, ident: &Ident) -> Vec<CodeRef> {
    match ident.kind {
        IdentKind::Path => resolve_path(graph, &ident.text).into_iter().collect(),
        IdentKind::Symbol => resolve_symbol(graph, &ident.text),
    }
}

/// The file the graph knows under `text`: an exact path, else the shortest path that ends with it.
fn resolve_path(graph: &CodeGraph, text: &str) -> Option<CodeRef> {
    let norm = CodeGraph::normalize_rel_path(text);
    let found = if graph.file_hashes.contains_key(&norm) {
        Some(norm)
    } else {
        let suffix = format!("/{}", norm.trim_start_matches('/'));
        graph.file_hashes.keys().filter(|f| f.ends_with(&suffix)).min_by_key(|f| f.len()).cloned()
    }?;
    Some(CodeRef { name: found.clone(), kind: "file".to_owned(), file: found, line: None })
}

fn resolve_symbol(graph: &CodeGraph, text: &str) -> Vec<CodeRef> {
    for name in name_variants(text) {
        let mut refs: Vec<CodeRef> = graph
            .symbol_index
            .get(&name)
            .into_iter()
            .flatten()
            .filter_map(|&id| graph.get_node(id))
            .filter(|n| n.kind == NodeKind::Symbol)
            .map(|n| CodeRef {
                name: n.label.clone(),
                kind: n
                    .symbol_kind
                    .map_or("symbol".to_owned(), |k| format!("{k:?}").to_lowercase()),
                file: n.file.clone(),
                line: Some(n.line),
            })
            .collect();
        refs.sort_by_key(|r| is_test_path(&r.file));
        refs.truncate(PER_IDENT);
        if !refs.is_empty() {
            return refs;
        }
    }
    Vec::new()
}

/// The spellings a name may have in code: as written, then PascalCase and camelCase of its parts
/// (`order-item` -> `OrderItem`), then each dotted segment (`Order.total` -> `Order`, `total`).
fn name_variants(text: &str) -> Vec<String> {
    let parts: Vec<&str> = text.split(['_', '-', '.']).filter(|p| !p.is_empty()).collect();
    let mut variants = vec![text.to_owned()];
    if parts.len() > 1 {
        let pascal: String = parts.iter().map(|p| capitalize(p)).collect();
        let mut camel = pascal.clone();
        if let Some(first) = camel.get_mut(..1) {
            first.make_ascii_lowercase();
        }
        variants.push(pascal);
        variants.push(camel);
        if text.contains('.') {
            variants.extend(parts.iter().map(|p| (*p).to_owned()));
        }
    }
    variants
}

fn capitalize(part: &str) -> String {
    let mut chars = part.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

pub(super) fn is_test_path(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    lower.contains("test") || lower.contains("spec") || lower.contains("__mocks__")
}

/// Path segments or dot-separated name parts (`order.mock.ts`) of data-setup files: a feature is not built there.
const SETUP_NAMES: &[&str] = &[
    "seed",
    "seeds",
    "fixture",
    "fixtures",
    "mock",
    "mocks",
    "__mocks__",
    "migration",
    "migrations",
];

/// Tests and data-setup files (seeds, fixtures, mocks, migrations): never offered as context.
/// A mapper or DTO is not here on purpose; that is where a new field goes.
pub(super) fn is_noise_path(file: &str) -> bool {
    is_test_path(file)
        || file
            .split(['/', '\\', '.'])
            .any(|part| SETUP_NAMES.contains(&part.to_ascii_lowercase().as_str()))
}

/// Distinct `words` that appear among the words of `text`.
fn overlap(text: &str, words: &[String]) -> usize {
    let tokens = tokens(text);
    words.iter().filter(|w| tokens.contains(w)).count()
}

/// Symbols and files whose own name (not their folder) shares two words with the prompt, or is
/// one of its longer words. Tests are left out: they echo the words of what they test.
fn content_code(run: &Run<'_>, out: &mut Gathered) {
    let (Some(graph), false) = (&run.graph, run.terms.words.is_empty()) else {
        return;
    };
    let words = &run.terms.words;
    let hits = query::query(graph, &words.join(" "), CANDIDATES * 2);
    let mut strong: Vec<(usize, query::QueryHit)> = hits
        .into_iter()
        .filter(|h| matches!(h.kind, NodeKind::Symbol | NodeKind::File) && !is_noise_path(&h.file))
        .filter_map(|h| {
            let shared = overlap(&h.label, words);
            let named = words.iter().any(|w| fold_accents(&h.label) == *w)
                && has_long_word(&h.label)
                && !matches!(node_kind(graph, &h).as_str(), "variable" | "constant" | "unknown");
            (shared >= 2 || named).then_some((shared, h))
        })
        .collect();
    strong.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.score.total_cmp(&a.1.score)));
    let room = run.limits.code.min(out.code.len() + MAX_CONTENT_CODE);
    for (_, hit) in strong {
        let kind = node_kind(graph, &hit);
        let line = (hit.kind == NodeKind::Symbol).then_some(hit.line);
        push_code(out, CodeRef { name: hit.label, kind, file: hit.file, line }, room);
    }
}

fn node_kind(graph: &CodeGraph, hit: &query::QueryHit) -> String {
    let symbol_kind = graph.node_by_id(&hit.id).and_then(|(_, node)| node.symbol_kind);
    match (hit.kind, symbol_kind) {
        (NodeKind::File, _) => "file".to_owned(),
        (_, Some(kind)) => format!("{kind:?}").to_lowercase(),
        (_, None) => "symbol".to_owned(),
    }
}

fn has_long_word(label: &str) -> bool {
    label.chars().count() >= SINGLE_WORD_MIN_CHARS
}

fn content_docs(run: &Run<'_>, out: &mut Gathered) {
    let words = run.terms.all_words();
    if words.is_empty() {
        return;
    }
    let bundles = bundle::existing(run.root, run.config);
    if bundles.is_empty() {
        return;
    }
    let concepts = concept::scan_all(run.root, &bundles, true);
    let hits = docs_search::search(&concepts, &words.join(" "), CANDIDATES);
    let strong = hits.into_iter().filter(|h| {
        let text = format!("{} {} {}", h.id, h.title.as_deref().unwrap_or_default(), h.snippet);
        overlap(&text, &words) >= 2
    });
    let mut drift = Drift::new(run.root);
    for hit in strong.take(run.limits.docs) {
        let stale = !run.out_of_time()
            && concept::find(&concepts, &hit.id).is_some_and(|c| is_stale(&mut drift, c));
        out.docs.push(DocRef { id: hit.id, title: hit.title, path: hit.path, stale });
    }
}

fn is_stale(drift: &mut Drift<'_>, concept: &Concept) -> bool {
    drift.check(concept).is_stale()
}

fn content_memories(run: &Run<'_>, out: &mut Gathered) {
    let words = run.terms.all_words();
    if words.is_empty() || !run.root.join(".harness/.state/anvil.db").is_file() {
        return;
    }
    let author = resolve_author_fast(run.root).unwrap_or_else(|_| "hook".to_owned());
    let engine = match MemoryEngine::open_without_sync(run.root, author) {
        Ok(engine) => engine,
        Err(err) => {
            out.problems.push(format!("memory unreadable: {err:#}"));
            return;
        }
    };
    match engine.search(&words.join(" "), None, CANDIDATES) {
        Ok(hits) => {
            let strong = hits.into_iter().filter(|h| {
                overlap(&format!("{} {} {}", h.title, h.topic_key, h.snippet), &words) >= 2
            });
            for hit in strong.take(run.limits.memories) {
                out.memories.push(MemoryRef {
                    id: hit.id,
                    title: hit.title,
                    kind: hit.kind.as_str().to_owned(),
                    stale: hit.freshness == FreshnessState::Stale,
                });
            }
        }
        Err(err) => out.problems.push(format!("memory search failed: {err:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_variants_cover_the_spellings_code_uses() {
        assert_eq!(name_variants("search_repo"), ["search_repo", "SearchRepo", "searchRepo"]);
        assert_eq!(name_variants("order-item")[1], "OrderItem");
        assert_eq!(name_variants("Order.total").last().map(String::as_str), Some("total"));
        assert_eq!(name_variants("Order"), ["Order"]);
    }

    #[test]
    fn overlap_counts_distinct_shared_words_ignoring_accents_and_case() {
        let words = vec!["descuento".to_owned(), "carrito".to_owned(), "login".to_owned()];
        assert_eq!(overlap("Descuento del Carrito", &words), 2);
        assert_eq!(overlap("calcularDescuento", &words), 1);
        assert_eq!(overlap("calcular", &words), 0);
    }

    #[test]
    fn seed_fixture_mock_and_migration_files_are_dropped_but_domain_files_are_kept() {
        for noise in [
            "orders-api/src/seed.ts",
            "src/seeds/orders.ts",
            "src/shared/database/migrations/1700000000000-initial-schema.ts",
            "test/fixtures/order.json",
            "src/__mocks__/order.ts",
            "src/orders/order.mock.ts",
            "src/orders/order.spec.ts",
        ] {
            assert!(is_noise_path(noise), "{noise}");
        }
        for kept in [
            "orders-api/src/orders/domain/order.ts",
            "orders-api/src/orders/infrastructure/order.mapper.ts",
            "src/seedling/plant.ts",
        ] {
            assert!(!is_noise_path(kept), "{kept}");
        }
    }

    #[test]
    fn test_paths_sort_after_source_paths() {
        assert!(is_test_path("src/order.spec.ts"));
        assert!(!is_test_path("src/order.ts"));
    }
}
