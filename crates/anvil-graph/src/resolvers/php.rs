//! PHP PSR-4 autoload, Laravel Blade/Inertia/Livewire, and symbol edge resolution.

use std::fs;
use std::path::Path;

use anvil_core::traits::{EdgeKind, EdgeRef};
use serde_json::Value;

use crate::model::{CodeGraph, NodeId};
use crate::resolvers::pick_preferred_symbol;

/// A single PSR-4 namespace-to-directory mapping loaded from a `composer.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Psr4Mapping {
    /// Subrepo prefix relative to workspace root (`""` for root, `"backend"` for `backend/composer.json`).
    pub subrepo_prefix: String,
    /// Namespace prefix ending with `\\` (e.g. `"App\\"`).
    pub namespace_prefix: String,
    /// Target directory relative to workspace root without trailing slash (e.g. `"backend/app"`).
    pub base_dir: String,
}

/// Loads PSR-4 mappings from root and child `composer.json` manifests (up to depth 2).
pub fn load_php_psr4(repo_root: &Path) -> Vec<Psr4Mapping> {
    let mut mappings = Vec::new();
    load_composer_in_dir(repo_root, "", &mut mappings);
    scan_child_composers(repo_root, repo_root, 0, 2, &mut mappings);
    mappings.sort_by(|a, b| {
        b.namespace_prefix
            .len()
            .cmp(&a.namespace_prefix.len())
            .then_with(|| a.subrepo_prefix.cmp(&b.subrepo_prefix))
    });
    mappings
}

fn scan_child_composers(
    root: &Path,
    dir: &Path,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<Psr4Mapping>,
) {
    if depth >= max_depth {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.')
            || matches!(name, "vendor" | "node_modules" | "target" | "dist" | "build" | "storage")
        {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if path.join("composer.json").is_file() {
            load_composer_in_dir(&path, &rel, out);
        } else {
            scan_child_composers(root, &path, depth + 1, max_depth, out);
        }
    }
}

fn load_composer_in_dir(dir: &Path, subrepo_prefix: &str, out: &mut Vec<Psr4Mapping>) {
    let path = dir.join("composer.json");
    let Ok(raw) = fs::read_to_string(&path) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<Value>(raw.strip_prefix('\u{feff}').unwrap_or(&raw)) else {
        return;
    };
    for section in ["autoload", "autoload-dev"] {
        let Some(psr4) = json
            .get(section)
            .and_then(|v| v.get("psr-4"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (ns, dir_val) in psr4 {
            let dirs: Vec<&str> = match dir_val {
                Value::String(s) => vec![s.as_str()],
                Value::Array(arr) => arr.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            for d in dirs {
                let clean_dir = d.trim().replace('\\', "/");
                let clean_dir = clean_dir.trim_matches('/');
                let base_dir = if subrepo_prefix.is_empty() {
                    clean_dir.to_owned()
                } else if clean_dir.is_empty() {
                    subrepo_prefix.to_owned()
                } else {
                    format!("{subrepo_prefix}/{clean_dir}")
                };
                let mut ns_prefix = ns.trim_start_matches('\\').to_owned();
                if !ns_prefix.is_empty() && !ns_prefix.ends_with('\\') {
                    ns_prefix.push('\\');
                }
                out.push(Psr4Mapping {
                    subrepo_prefix: subrepo_prefix.to_owned(),
                    namespace_prefix: ns_prefix,
                    base_dir,
                });
            }
        }
    }
}

/// Resolves PHP imports, PSR-4 class references, Controller::method calls, DI, and Laravel templates.
pub fn resolve_php_edge(
    edge: &EdgeRef,
    file_imports: &[String],
    graph: &CodeGraph,
    psr4: &[Psr4Mapping],
) -> Option<NodeId> {
    match edge.kind {
        EdgeKind::Imports => resolve_psr4_fqn(&edge.target_path_or_symbol, &edge.source_file, graph, psr4),
        EdgeKind::RendersTemplate => resolve_laravel_template(&edge.target_path_or_symbol, &edge.source_file, graph, psr4),
        EdgeKind::Calls
        | EdgeKind::Extends
        | EdgeKind::Implements
        | EdgeKind::Injects
        | EdgeKind::Provides => resolve_php_symbol_target(
            &edge.target_path_or_symbol,
            &edge.source_file,
            file_imports,
            graph,
            psr4,
        ),
        _ => None,
    }
}

/// Resolves Blade template `@extends`, `@include`, `<x-...>`, and `<livewire:...>` edges.
pub fn resolve_blade_edge(
    edge: &EdgeRef,
    graph: &CodeGraph,
    psr4: &[Psr4Mapping],
) -> Option<NodeId> {
    if edge.kind == EdgeKind::RendersTemplate {
        return resolve_laravel_template(&edge.target_path_or_symbol, &edge.source_file, graph, psr4);
    }
    None
}

fn resolve_php_symbol_target(
    raw_target: &str,
    source_file: &str,
    file_imports: &[String],
    graph: &CodeGraph,
    psr4: &[Psr4Mapping],
) -> Option<NodeId> {
    let target = raw_target.trim().trim_start_matches('\\');
    if target.is_empty() {
        return None;
    }

    if let Some((cls_part, member)) = target.split_once("::") {
        let cls_short = cls_part.rsplit('\\').next().unwrap_or(cls_part);
        let cls_nid = if cls_part.contains('\\') {
            resolve_psr4_fqn(cls_part, source_file, graph, psr4)
        } else {
            pick_preferred_symbol(cls_short, source_file, file_imports, graph)
                .or_else(|| resolve_same_dir_php_class(cls_short, source_file, graph))
        }?;
        if let Some(cls_node) = graph.get_node(cls_nid)
            && !cls_node.file.is_empty()
        {
            let qualified = format!("{cls_short}::{member}");
            let method_id = CodeGraph::symbol_node_id(&cls_node.file, &qualified);
            if let Some((mid, _)) = graph.node_by_id(&method_id) {
                return Some(mid);
            }
        }
        return Some(cls_nid);
    }

    if target.contains('\\')
        && let Some(hit) = resolve_psr4_fqn(target, source_file, graph, psr4)
    {
        return Some(hit);
    }

    let short = target.rsplit('\\').next().unwrap_or(target);
    pick_preferred_symbol(short, source_file, file_imports, graph)
        .or_else(|| resolve_same_dir_php_class(short, source_file, graph))
}

fn resolve_same_dir_php_class(
    class_name: &str,
    source_file: &str,
    graph: &CodeGraph,
) -> Option<NodeId> {
    let parent = source_file.rsplit_once('/').map_or("", |(d, _)| d);
    let cand_file = if parent.is_empty() {
        format!("{class_name}.php")
    } else {
        format!("{parent}/{class_name}.php")
    };
    if !graph.file_hashes.contains_key(&cand_file) {
        return None;
    }
    let sym_id = CodeGraph::symbol_node_id(&cand_file, class_name);
    graph
        .node_by_id(&sym_id)
        .or_else(|| graph.node_by_id(&CodeGraph::file_node_id(&cand_file)))
        .map(|(nid, _)| nid)
}

fn resolve_psr4_fqn(
    fqn_raw: &str,
    source_file: &str,
    graph: &CodeGraph,
    psr4: &[Psr4Mapping],
) -> Option<NodeId> {
    let fqn = fqn_raw.trim().trim_start_matches('\\');
    if fqn.is_empty() {
        return None;
    }
    let src_subrepo = detect_source_subrepo(source_file, psr4);

    // Try mappings belonging to the same subrepo first, then all others.
    for same_subrepo in [true, false] {
        for mapping in psr4 {
            let is_same = mapping.subrepo_prefix == src_subrepo;
            if is_same != same_subrepo {
                continue;
            }
            let Some(rest) = fqn.strip_prefix(&mapping.namespace_prefix) else {
                continue;
            };
            let rel_php = format!("{}.php", rest.replace('\\', "/"));
            let candidate = if mapping.base_dir.is_empty() {
                rel_php
            } else {
                format!("{}/{rel_php}", mapping.base_dir)
            };
            if graph.file_hashes.contains_key(&candidate) {
                let short = fqn.rsplit('\\').next().unwrap_or(fqn);
                let sym_id = CodeGraph::symbol_node_id(&candidate, short);
                return graph
                    .node_by_id(&sym_id)
                    .or_else(|| graph.node_by_id(&CodeGraph::file_node_id(&candidate)))
                    .map(|(nid, _)| nid);
            }
        }
    }
    None
}

fn detect_source_subrepo<'a>(source_file: &str, psr4: &'a [Psr4Mapping]) -> &'a str {
    let mut best = "";
    for m in psr4 {
        if !m.subrepo_prefix.is_empty() {
            let with_slash = format!("{}/", m.subrepo_prefix);
            if source_file.starts_with(&with_slash) && m.subrepo_prefix.len() > best.len() {
                best = &m.subrepo_prefix;
            }
        }
    }
    best
}

fn resolve_laravel_template(
    target: &str,
    source_file: &str,
    graph: &CodeGraph,
    psr4: &[Psr4Mapping],
) -> Option<NodeId> {
    let subrepo = detect_source_subrepo(source_file, psr4);
    if let Some(view_name) = target.strip_prefix("blade:") {
        let bare = view_name.rsplit("::").next().unwrap_or(view_name);
        let rel = bare.replace('.', "/");
        return file_node_if_present(&join_subrepo(subrepo, &format!("resources/views/{rel}.blade.php")), graph);
    }
    if let Some(comp) = target.strip_prefix("blade_component:") {
        return resolve_blade_component(subrepo, comp, graph);
    }
    if let Some(page) = target.strip_prefix("inertia:") {
        return resolve_inertia_page(subrepo, page, graph);
    }
    if let Some(lw) = target.strip_prefix("livewire:") {
        return resolve_livewire_target(subrepo, lw, graph);
    }
    None
}

fn resolve_blade_component(subrepo: &str, comp: &str, graph: &CodeGraph) -> Option<NodeId> {
    let rel = comp.replace('.', "/");
    let cands = [
        join_subrepo(subrepo, &format!("resources/views/components/{rel}.blade.php")),
        join_subrepo(subrepo, &format!("resources/views/components/{rel}/index.blade.php")),
        join_subrepo(subrepo, &format!("app/View/Components/{}.php", to_studly_path(comp))),
    ];
    cands.into_iter().find_map(|c| file_node_if_present(&c, graph))
}

fn resolve_inertia_page(subrepo: &str, page: &str, graph: &CodeGraph) -> Option<NodeId> {
    let clean = page.trim_matches('/');
    for base in ["resources/js/Pages", "resources/js/pages", "src/Pages", "src/pages"] {
        for ext in ["tsx", "vue", "jsx", "ts", "js"] {
            let exact = join_subrepo(subrepo, &format!("{base}/{clean}.{ext}"));
            if let Some(nid) = file_node_if_present(&exact, graph) {
                return Some(nid);
            }
            let suffix = format!("/{base}/{clean}.{ext}");
            if let Some(hit) = graph.file_hashes.keys().find(|f| f.ends_with(&suffix))
                && let Some(nid) = file_node_if_present(hit, graph)
            {
                return Some(nid);
            }
        }
    }
    None
}

fn resolve_livewire_target(subrepo: &str, lw: &str, graph: &CodeGraph) -> Option<NodeId> {
    let studly = to_studly_path(lw);
    let rel = lw.replace('.', "/");
    let cands = [
        join_subrepo(subrepo, &format!("app/Livewire/{studly}.php")),
        join_subrepo(subrepo, &format!("app/Http/Livewire/{studly}.php")),
        join_subrepo(subrepo, &format!("resources/views/livewire/{rel}.blade.php")),
    ];
    cands.into_iter().find_map(|c| file_node_if_present(&c, graph))
}

fn file_node_if_present(rel_path: &str, graph: &CodeGraph) -> Option<NodeId> {
    graph
        .file_hashes
        .contains_key(rel_path)
        .then(|| graph.node_by_id(&CodeGraph::file_node_id(rel_path)).map(|(nid, _)| nid))
        .flatten()
}

fn join_subrepo(subrepo: &str, rel: &str) -> String {
    if subrepo.is_empty() {
        rel.to_owned()
    } else {
        format!("{subrepo}/{rel}")
    }
}

fn to_studly_path(raw: &str) -> String {
    raw.split(['.', '/'])
        .filter(|s| !s.is_empty())
        .map(|seg| {
            seg.split(['-', '_'])
                .filter(|w| !w.is_empty())
                .map(|w| {
                    let mut chars = w.chars();
                    match chars.next() {
                        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                        None => String::new(),
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}
