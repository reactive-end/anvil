//! Tier 1 guard: new code that no non-test file uses.
//!
//! The graph only records file-level imports plus name-matched calls, so a graph edge is treated as proof
//! of use, and a missing edge is confirmed with a word search in the other non-test files before anything
//! is reported. Anything the graph cannot see (entry points, frameworks, macros, unresolved languages) is
//! left alone.

mod conventions;
mod text;

use std::path::Path;

use anvil_core::git::AddedLines;
use anvil_core::traits::{Language, SymbolKind};
use rustc_hash::FxHashSet;
use serde::Serialize;

use crate::impact::sibling::is_test_file;
use crate::model::{CodeGraph, CompactEdge, GraphEdgeKind, GraphNode, NodeId, NodeKind};

use text::Corpus;

/// Rule id printed with every finding.
pub const RULE: &str = "overengineering:unused-new-code";
const SUGGESTION: &str = "wire it into its caller in this change, delete it, or add anvil-allow(overengineering): <reason>";

/// New code with no consumer in non-test code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverengineeringFinding {
    pub rule: &'static str,
    pub file: String,
    pub line: u32,
    /// `None` when the whole file is unused.
    pub symbol: Option<String>,
    pub message: String,
    pub suggestion: &'static str,
}

/// Report from `check_overengineering`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverengineeringReport {
    pub flagged: bool,
    pub findings: Vec<OverengineeringFinding>,
}

/// Flags new code that no other non-test file uses: added files, and exported symbols defined on added
/// lines (`added_lines`) of added or modified files. Symbols that predate the change are never reported.
pub fn check_overengineering(
    graph: &CodeGraph,
    added_files: &[String],
    added_lines: &AddedLines,
) -> OverengineeringReport {
    let mut added: Vec<String> =
        added_files.iter().map(|f| CodeGraph::normalize_rel_path(f)).collect();
    added.sort();
    added.dedup();
    let mut modified: Vec<(&String, &Vec<(u32, u32)>)> =
        added_lines.iter().filter(|(file, _)| !added.contains(file)).collect();
    modified.sort();
    let guard = Guard::new(graph);
    let new_files = added.iter().flat_map(|file| guard.check_file(file, None));
    let touched = modified.iter().flat_map(|(file, ranges)| guard.check_file(file, Some(ranges)));
    let findings: Vec<OverengineeringFinding> = new_files.chain(touched).collect();
    OverengineeringReport { flagged: !findings.is_empty(), findings }
}

struct Guard<'a> {
    graph: &'a CodeGraph,
    root: &'a Path,
    active: FxHashSet<Language>,
    corpus: Corpus,
}

/// An exported symbol the guard may report.
struct Candidate<'a> {
    nid: NodeId,
    node: &'a GraphNode,
}

impl<'a> Guard<'a> {
    fn new(graph: &'a CodeGraph) -> Self {
        Self {
            graph,
            root: Path::new(&graph.repo_root),
            active: resolving_languages(graph),
            corpus: Corpus::default(),
        }
    }

    /// `new_lines` is `None` for an added file and the added ranges for a modified one.
    fn check_file(
        &self,
        file: &str,
        new_lines: Option<&[(u32, u32)]>,
    ) -> Vec<OverengineeringFinding> {
        let Some(language) = self.file_language(file) else {
            return Vec::new();
        };
        if !conventions::is_checked_language(language)
            || !self.active.contains(&language)
            || conventions::is_ignored_path(file)
            || conventions::is_entry_point(self.root, file, language)
        {
            return Vec::new();
        }
        let Ok(source) = std::fs::read_to_string(self.root.join(file)) else {
            return Vec::new();
        };
        if conventions::is_framework_managed(language, &source)
            || text::allows_file(&source)
            || self.is_test_support(file)
        {
            return Vec::new();
        }
        let mut candidates = self.candidates(file);
        if let Some(ranges) = new_lines {
            candidates.retain(|c| {
                ranges.iter().any(|&(first, last)| (first..=last).contains(&c.node.line))
            });
        }
        if candidates.is_empty()
            || candidates.iter().any(|c| conventions::is_macro_reached(language, &source, c.node))
        {
            return Vec::new();
        }
        let whole_file_is_new = new_lines.is_none();
        if whole_file_is_new
            && self.file_consumers(file) == 0
            && !self.any_mentioned_elsewhere(file, &candidates)
        {
            return vec![self.whole_file_finding(file, &candidates)];
        }
        self.unused_symbols(file, &source, &candidates)
    }

    fn file_language(&self, file: &str) -> Option<Language> {
        let nids = self.graph.file_nodes.get(file)?;
        nids.iter()
            .filter_map(|&nid| self.graph.get_node(nid))
            .find(|n| n.kind == NodeKind::File)
            .and_then(|n| n.language)
    }

    fn candidates(&self, file: &str) -> Vec<Candidate<'a>> {
        let nids = self.graph.file_nodes.get(file).map(Vec::as_slice).unwrap_or_default();
        nids.iter()
            .filter_map(|&nid| self.graph.get_node(nid).map(|node| Candidate { nid, node }))
            .filter(|c| c.node.kind == NodeKind::Symbol && c.node.exported && is_reportable(c.node))
            .collect()
    }

    /// Distinct non-test files that reach `file` through a resolved edge.
    fn file_consumers(&self, file: &str) -> usize {
        let nids = self.graph.file_nodes.get(file).map(Vec::as_slice).unwrap_or_default();
        let mut consumers: FxHashSet<&str> = FxHashSet::default();
        for &nid in nids {
            for edge in self.graph.incoming_edges(nid) {
                if let Some(src) = self.consumer_source(edge, file) {
                    consumers.insert(src);
                }
            }
        }
        consumers.len()
    }

    /// The non-test file an edge comes from, unless the edge is not a real use of `file`.
    fn consumer_source(&self, edge: &CompactEdge, file: &str) -> Option<&'a str> {
        if !edge.is_resolved()
            || edge.kind == GraphEdgeKind::Contains
            || is_mod_declaration(self.graph, edge)
        {
            return None;
        }
        let src = self.graph.get_node(edge.source)?;
        let counts = src.kind != NodeKind::Semantic
            && !src.file.is_empty()
            && src.file != file
            && !conventions::is_test_like(&src.file);
        counts.then_some(src.file.as_str())
    }

    /// A helper shared by other files' tests (`kit.ts` imported by `a.test.ts` and `b.test.ts`) is test
    /// code. A file whose only consumer is its own test (`x.ts` and `x.test.ts`) is not.
    fn is_test_support(&self, file: &str) -> bool {
        let stem = conventions::base_stem(file);
        let nids = self.graph.file_nodes.get(file).map(Vec::as_slice).unwrap_or_default();
        let tests: Vec<&str> = nids
            .iter()
            .flat_map(|&nid| self.graph.incoming_edges(nid))
            .filter(|edge| edge.is_resolved() && edge.kind != GraphEdgeKind::Contains)
            .filter_map(|edge| self.graph.get_node(edge.source))
            .filter(|src| src.file != file && conventions::is_test_like(&src.file))
            .map(|src| src.file.as_str())
            .collect();
        !tests.is_empty()
            && tests.iter().all(|t| conventions::base_stem(t) != stem)
            && self.file_consumers(file) == 0
    }

    fn any_mentioned_elsewhere(&self, file: &str, candidates: &[Candidate<'_>]) -> bool {
        candidates.iter().any(|c| self.corpus.mentioned_outside(self.graph, file, &c.node.label))
    }

    fn whole_file_finding(
        &self,
        file: &str,
        candidates: &[Candidate<'_>],
    ) -> OverengineeringFinding {
        let line = candidates.iter().map(|c| c.node.line).min().unwrap_or(1);
        let names: Vec<&str> = candidates.iter().map(|c| c.node.label.as_str()).collect();
        OverengineeringFinding {
            rule: RULE,
            file: file.to_owned(),
            line,
            symbol: None,
            message: format!(
                "new file `{file}` has no non-test consumer (defines {})",
                names.join(", ")
            ),
            suggestion: SUGGESTION,
        }
    }

    fn unused_symbols(
        &self,
        file: &str,
        source: &str,
        candidates: &[Candidate<'_>],
    ) -> Vec<OverengineeringFinding> {
        candidates
            .iter()
            .filter(|c| !self.symbol_is_used(file, source, c))
            .map(|c| OverengineeringFinding {
                rule: RULE,
                file: file.to_owned(),
                line: c.node.line,
                symbol: Some(c.node.label.clone()),
                message: format!("exported `{}` is new and has no non-test consumer", c.node.label),
                suggestion: SUGGESTION,
            })
            .collect()
    }

    fn symbol_is_used(&self, file: &str, source: &str, c: &Candidate<'_>) -> bool {
        let name = &c.node.label;
        text::allows_symbol(source, c.node.line)
            || self.has_graph_consumer(c.nid)
            || text::count_word(source, name) > 1
            || self.corpus.mentioned_outside(self.graph, file, name)
    }

    fn has_graph_consumer(&self, nid: NodeId) -> bool {
        self.graph.incoming_edges(nid).into_iter().any(|edge| {
            edge.is_resolved()
                && edge.kind != GraphEdgeKind::Contains
                && self.graph.get_node(edge.source).is_some_and(|src| {
                    src.kind != NodeKind::Semantic && !conventions::is_test_like(&src.file)
                })
        })
    }
}

/// Kinds a name search can vouch for. Methods, impl blocks, modules, and variables are reached through
/// traits, receivers, `mod` declarations, or frameworks that the graph does not model.
fn is_reportable(node: &GraphNode) -> bool {
    let kind_ok = matches!(
        node.symbol_kind,
        Some(
            SymbolKind::Function
                | SymbolKind::Class
                | SymbolKind::Interface
                | SymbolKind::TypeAlias
                | SymbolKind::Component
                | SymbolKind::Constant
        )
    );
    kind_ok && !matches!(node.label.as_str(), "main" | "default" | "")
}

/// A Rust `mod foo;` line: the parent file imports the child file at the line of its `foo` module symbol.
fn is_mod_declaration(graph: &CodeGraph, edge: &CompactEdge) -> bool {
    if edge.kind != GraphEdgeKind::Imports {
        return false;
    }
    let (Some(src), Some(tgt)) = (graph.get_node(edge.source), graph.get_node(edge.target)) else {
        return false;
    };
    if src.kind != NodeKind::File
        || tgt.kind != NodeKind::File
        || src.language != Some(Language::Rust)
    {
        return false;
    }
    graph.file_nodes.get(&src.file).is_some_and(|nids| {
        nids.iter()
            .filter_map(|&nid| graph.get_node(nid))
            .any(|n| n.symbol_kind == Some(SymbolKind::Module) && n.line == edge.line)
    })
}

/// Languages that have at least one resolved cross-file reference into them. Where none exists the
/// resolver is not working for that repository, and silence beats guessing.
fn resolving_languages(graph: &CodeGraph) -> FxHashSet<Language> {
    let mut active = FxHashSet::default();
    for edge in graph.edges.iter().filter(|e| e.is_resolved() && e.kind != GraphEdgeKind::Contains)
    {
        if is_mod_declaration(graph, edge) {
            continue;
        }
        let (Some(src), Some(tgt)) = (graph.get_node(edge.source), graph.get_node(edge.target))
        else {
            continue;
        };
        if !src.file.is_empty()
            && src.file != tgt.file
            && !is_test_file(&src.file)
            && let Some(language) = tgt.language
        {
            active.insert(language);
        }
    }
    active
}
