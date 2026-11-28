//! BM25 search over title, description, tags, headings, and body of concepts.

use std::path::Path;

use anvil_core::config::AnvilConfig;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;

use crate::bundle;
use crate::concept::{Concept, scan_all};

const K1: f64 = 1.2;
const B: f64 = 0.75;
const SNIPPET_CHARS: usize = 160;

/// One search result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchHit {
    /// Concept key, e.g. `orders-api/orders`.
    pub id: String,
    /// Repo-relative path of the concept file.
    pub path: String,
    pub title: Option<String>,
    pub score: f64,
    pub snippet: String,
}

/// Lowercase alphanumeric words of `text`.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// A concept reduced to weighted term frequencies.
struct Indexed<'a> {
    concept: &'a Concept,
    terms: FxHashMap<String, f64>,
    length: f64,
}

fn index(concept: &Concept) -> Indexed<'_> {
    let doc = &concept.doc;
    let headings: Vec<&str> = doc
        .body()
        .lines()
        .filter(|l| l.starts_with('#'))
        .map(|l| l.trim_start_matches('#'))
        .collect();
    let tags = doc.tags().join(" ");
    let fields: [(f64, String); 6] = [
        (3.0, doc.title().unwrap_or_default().to_owned()),
        (3.0, concept.key()),
        (3.0, tags),
        (2.0, doc.description().unwrap_or_default().to_owned()),
        (2.0, headings.join(" ")),
        (1.0, doc.body().to_owned()),
    ];
    let mut terms: FxHashMap<String, f64> = FxHashMap::default();
    let mut length = 0.0;
    for (weight, text) in &fields {
        for token in tokenize(text) {
            *terms.entry(token).or_default() += weight;
            length += weight;
        }
    }
    Indexed { concept, terms, length }
}

/// Ranks `concepts` against `query` and returns the best `limit` hits, best first.
pub fn search(concepts: &[Concept], query: &str, limit: usize) -> Vec<SearchHit> {
    let wanted: Vec<String> =
        tokenize(query).into_iter().collect::<FxHashSet<_>>().into_iter().collect();
    if wanted.is_empty() || concepts.is_empty() {
        return Vec::new();
    }
    let indexed: Vec<Indexed<'_>> = concepts.iter().map(index).collect();
    let total = indexed.len() as f64;
    let average = (indexed.iter().map(|i| i.length).sum::<f64>() / total).max(1.0);
    let mut hits: Vec<SearchHit> = Vec::new();
    for item in &indexed {
        let mut score = 0.0;
        for term in &wanted {
            let Some(tf) = item.terms.get(term) else {
                continue;
            };
            let df = indexed.iter().filter(|i| i.terms.contains_key(term)).count() as f64;
            let idf = (1.0 + (total - df + 0.5) / (df + 0.5)).ln();
            score += idf * (tf * (K1 + 1.0)) / (tf + K1 * (1.0 - B + B * item.length / average));
        }
        if score > 0.0 {
            hits.push(hit_of(item.concept, score, &wanted));
        }
    }
    hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    hits.truncate(limit);
    hits
}

fn hit_of(concept: &Concept, score: f64, wanted: &[String]) -> SearchHit {
    SearchHit {
        id: concept.key(),
        path: concept.rel_path.clone(),
        title: concept.doc.title().map(str::to_owned),
        score: (score * 1000.0).round() / 1000.0,
        snippet: snippet(concept, wanted),
    }
}

fn snippet(concept: &Concept, wanted: &[String]) -> String {
    let matching = concept.doc.body().lines().map(str::trim).find(|line| {
        !line.is_empty()
            && !line.starts_with("<!--")
            && tokenize(line).iter().any(|t| wanted.contains(t))
    });
    let text = matching
        .or_else(|| concept.doc.description())
        .or_else(|| concept.doc.body().lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or_default();
    let clipped: String = text.chars().take(SNIPPET_CHARS).collect();
    if text.chars().count() > SNIPPET_CHARS { format!("{clipped}...") } else { clipped }
}

/// Searches every bundle of the repository: the entry point other tools (for example a graph, docs,
/// code search cascade) call.
pub fn search_repo(root: &Path, config: &AnvilConfig, query: &str, limit: usize) -> Vec<SearchHit> {
    let bundles = bundle::existing(root, config);
    search(&scan_all(root, &bundles, true), query, limit)
}
