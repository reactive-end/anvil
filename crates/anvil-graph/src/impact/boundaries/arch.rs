//! Checks of generated architecture rules: layer imports, forbidden packages, and module isolation.
//!
//! These rules look at every import edge, internal files and external packages alike, and report the
//! line of the offending import. User `boundaries` keep their own matcher in the parent module.

use anvil_core::config::architecture::rules::{
    ArchRule, ArchRuleKind, ModuleIsolation, package_matches, package_name, violation_message,
};
use anvil_core::traits::{Diagnostic, Severity};
use rustc_hash::FxHashSet;

use super::glob_match;
use crate::model::{CodeGraph, GraphEdgeKind, NodeKind};

const JS_EXTENSIONS: &[&str] =
    &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "vue", "svelte"];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Target {
    /// A file of the repository.
    File(String),
    /// An import specifier the graph could not resolve to a file (a package, usually).
    External(String),
}

impl Target {
    fn text(&self) -> &str {
        match self {
            Self::File(path) | Self::External(path) => path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Import {
    file: String,
    line: u32,
    target: Target,
}

/// Appends one diagnostic per import that breaks a generated rule; `scope_files` empty means all files.
pub(super) fn collect_violations(
    graph: &CodeGraph,
    rules: &[ArchRule],
    scope_files: &[String],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let scope: FxHashSet<String> =
        scope_files.iter().map(|f| CodeGraph::normalize_rel_path(f)).collect();
    for import in collect_imports(graph) {
        if (!scope.is_empty() && !scope.contains(&import.file)) || is_test_source(&import.file) {
            continue;
        }
        for rule in rules.iter().filter(|rule| violates(rule, &import)) {
            diagnostics.push(Diagnostic {
                rule: format!("boundary:{}", rule.name),
                severity: Severity::Error,
                file: import.file.clone(),
                line: import.line,
                column: 1,
                message: violation_message(import.target.text(), &rule.name),
                suggestion: Some(rule.reason.clone()),
            });
        }
    }
}

fn collect_imports(graph: &CodeGraph) -> Vec<Import> {
    let mut imports = Vec::new();
    for edge in &graph.edges {
        if !matches!(edge.kind, GraphEdgeKind::Imports | GraphEdgeKind::ReExports) {
            continue;
        }
        let Some(src) = graph.get_node(edge.source) else {
            continue;
        };
        if src.kind == NodeKind::Semantic || src.file.is_empty() {
            continue;
        }
        let target = if edge.is_resolved() {
            match graph.get_node(edge.target) {
                Some(dst)
                    if dst.kind != NodeKind::Semantic
                        && !dst.file.is_empty()
                        && dst.file != src.file =>
                {
                    Target::File(dst.file.clone())
                }
                _ => continue,
            }
        } else if let Some(external) = graph.external_str(edge.target) {
            Target::External(external.to_owned())
        } else {
            continue;
        };
        imports.push(Import { file: src.file.clone(), line: edge.line, target });
    }
    imports.sort();
    imports.dedup();
    imports
}

fn violates(rule: &ArchRule, import: &Import) -> bool {
    match (&rule.kind, &import.target) {
        (ArchRuleKind::Layer { from, forbid }, Target::File(dst)) => {
            matches_any(from, &import.file) && matches_any(forbid, dst)
        }
        (ArchRuleKind::Packages { from, packages }, Target::External(spec)) => {
            is_js_source(&import.file)
                && matches_any(from, &import.file)
                && package_name(spec)
                    .is_some_and(|pkg| packages.iter().any(|p| package_matches(p, pkg)))
        }
        (ArchRuleKind::Modules(iso), Target::File(dst)) => crosses_module(iso, &import.file, dst),
        _ => false,
    }
}

fn matches_any(globs: &[String], path: &str) -> bool {
    globs.iter().any(|glob| glob_match(glob, path))
}

/// `true` when `src` and `dst` live in different modules and `dst` is not a public path of its module.
fn crosses_module(iso: &ModuleIsolation, src: &str, dst: &str) -> bool {
    if !iso.from.is_empty() && !matches_any(&iso.from, src) {
        return false;
    }
    let (Some(from), Some(to)) = (module_of(iso, src), module_of(iso, dst)) else {
        return false;
    };
    if from.root == to.root && from.name == to.name {
        return false;
    }
    // Shared kernels (`shared/`, `common/`, ...) hold app-wide wiring such as the database config that
    // registers every module's entities, so they are neither checked nor protected.
    let is_exempt = |name: &str| iso.exempt.iter().any(|exempt| exempt == name);
    !is_exempt(from.name) && !is_exempt(to.name) && !matches_any(&iso.public, &to.rel)
}

struct ModuleRef<'a> {
    root: &'a str,
    name: &'a str,
    /// Path inside the module (`domain/order.ts`).
    rel: String,
}

/// The module `path` belongs to: the longest matching root decides, and the path needs a directory and
/// a file below it (`src/orders/order.ts`) to count as inside a module.
fn module_of<'a>(iso: &'a ModuleIsolation, path: &'a str) -> Option<ModuleRef<'a>> {
    let mut roots: Vec<&str> = iso.roots.iter().map(String::as_str).collect();
    roots.sort_by_key(|root| std::cmp::Reverse(root.len()));
    for root in roots {
        let trimmed = root.trim_matches('/');
        let below = if trimmed.is_empty() || trimmed == "." {
            Some(path)
        } else {
            path.strip_prefix(trimmed).and_then(|rest| rest.strip_prefix('/'))
        };
        let Some(below) = below else {
            continue;
        };
        let (name, rel) = below.split_once('/')?;
        if !iso.only.is_empty() && !iso.only.iter().any(|m| m == name) {
            return None;
        }
        return Some(ModuleRef { root, name, rel: rel.to_owned() });
    }
    None
}

fn is_js_source(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| JS_EXTENSIONS.contains(&ext))
}

/// Specs, tests and e2e fixtures wire concrete adapters on purpose and are not held to the rules.
fn is_test_source(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.contains(".spec.")
        || file.contains(".test.")
        || path.split('/').any(|seg| matches!(seg, "__tests__" | "test" | "tests" | "e2e"))
}
