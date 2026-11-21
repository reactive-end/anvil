//! TypeScript/JavaScript resolution (`tsconfig.json` `baseUrl`, `paths`, relative & `index` probing).

use std::fs;
use std::path::{Path, PathBuf};

use anvil_core::fs_util::strip_jsonc;
use anvil_core::traits::{EdgeKind, EdgeRef};
use rustc_hash::FxHashSet;
use serde::Deserialize;

use crate::model::{CodeGraph, NodeId};
use crate::resolvers::{join_normalized, normalize_join_rel, pick_preferred_symbol};

const TS_EXTENSIONS: &[&str] = &[
    "", ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".json",
    "/index.ts", "/index.tsx", "/index.js", "/index.jsx",
];

/// Parsed `tsconfig*.json` resolution configuration.
#[derive(Debug, Clone, Default)]
pub struct TsConfigRules {
    pub base_url: Option<String>,
    pub paths: Vec<(String, Vec<String>)>,
}

#[derive(Deserialize)]
struct RawTsConfig {
    extends: Option<String>,
    #[serde(rename = "compilerOptions")]
    compiler_options: Option<RawCompilerOptions>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCompilerOptions {
    base_url: Option<String>,
    paths: Option<std::collections::HashMap<String, Vec<String>>>,
}

impl TsConfigRules {
    /// Loads `tsconfig.base.json` -> `tsconfig.json` -> `tsconfig.app.json` (child overrides base).
    pub fn load(repo_root: &Path) -> Self {
        let mut combined = Self::default();
        for name in ["tsconfig.base.json", "tsconfig.json", "tsconfig.app.json"] {
            let candidate = repo_root.join(name);
            if candidate.is_file() {
                let mut visited = FxHashSet::default();
                let rules = Self::load_file(repo_root, &candidate, &mut visited, 0);
                if rules.base_url.is_some() {
                    combined.base_url = rules.base_url;
                }
                for (pat, targets) in rules.paths {
                    upsert_path(&mut combined.paths, pat, targets);
                }
            }
        }
        combined.paths.sort_by_key(|a| std::cmp::Reverse(a.0.len()));
        combined
    }

    fn load_file(
        repo_root: &Path,
        path: &Path,
        visited: &mut FxHashSet<String>,
        depth: usize,
    ) -> Self {
        if depth > 5 {
            return Self::default();
        }
        let key = path.to_string_lossy().into_owned();
        if !visited.insert(key) {
            return Self::default();
        }
        let Ok(raw) = fs::read_to_string(path) else {
            return Self::default();
        };
        let cleaned = strip_jsonc(&raw);
        let Ok(parsed) = serde_json::from_str::<RawTsConfig>(&cleaned) else {
            return Self::default();
        };

        let mut rules = Self::default();
        if let Some(ext) = parsed.extends.as_deref()
            && let Some(parent_dir) = path.parent()
            && let Some(ep) = resolve_extends_path(repo_root, parent_dir, ext)
        {
            rules = Self::load_file(repo_root, &ep, visited, depth + 1);
        }

        if let Some(opts) = parsed.compiler_options {
            if let Some(base) = opts.base_url {
                let base_rel = path
                    .parent()
                    .and_then(|p| p.strip_prefix(repo_root).ok())
                    .map(|p| CodeGraph::normalize_rel_path(&p.to_string_lossy()))
                    .unwrap_or_default();
                rules.base_url = Some(normalize_join_rel(&base_rel, &base));
            }
            if let Some(paths) = opts.paths {
                for (pat, targets) in paths {
                    upsert_path(&mut rules.paths, pat, targets);
                }
            }
        }
        rules
    }
}

fn upsert_path(paths: &mut Vec<(String, Vec<String>)>, pat: String, targets: Vec<String>) {
    if let Some(existing) = paths.iter_mut().find(|(p, _)| p == &pat) {
        existing.1 = targets;
    } else {
        paths.push((pat, targets));
    }
}

fn resolve_extends_path(repo_root: &Path, dir: &Path, ext: &str) -> Option<PathBuf> {
    if ext.starts_with('.') || ext.starts_with('/') {
        return probe_tsconfig_candidate(&dir.join(ext));
    }
    if let Some(local) = probe_tsconfig_candidate(&dir.join(ext)) {
        return Some(local);
    }
    let mut cur = Some(dir);
    while let Some(c) = cur {
        let nm_candidate = c.join("node_modules").join(ext);
        if let Some(found) = probe_tsconfig_candidate(&nm_candidate) {
            return Some(found);
        }
        if c == repo_root {
            break;
        }
        cur = c.parent().filter(|p| p.starts_with(repo_root));
    }
    None
}

fn probe_tsconfig_candidate(base: &Path) -> Option<PathBuf> {
    if base.is_file() {
        return Some(base.to_path_buf());
    }
    let mut json_os = base.as_os_str().to_os_string();
    json_os.push(".json");
    let with_json = PathBuf::from(json_os);
    if with_json.is_file() {
        return Some(with_json);
    }
    let nested = base.join("tsconfig.json");
    nested.is_file().then_some(nested)
}

/// Resolves a TypeScript/JavaScript `EdgeRef` to a target `NodeId` when internal.
pub fn resolve_ts_edge(
    edge: &EdgeRef,
    file_imports: &[String],
    graph: &CodeGraph,
    rules: &TsConfigRules,
) -> Option<NodeId> {
    match edge.kind {
        EdgeKind::Imports | EdgeKind::ReExports => {
            resolve_module_specifier(&edge.source_file, &edge.target_path_or_symbol, graph, rules)
                .and_then(|rel| graph.node_by_id(&CodeGraph::file_node_id(&rel)).map(|(id, _)| id))
        }
        EdgeKind::UsesProp => {
            let component = edge.target_path_or_symbol.split('.').next().unwrap_or("");
            pick_preferred_symbol(component, &edge.source_file, file_imports, graph)
        }
        EdgeKind::Calls | EdgeKind::Extends | EdgeKind::Implements => {
            let sym = edge
                .target_path_or_symbol
                .split('<')
                .next()
                .unwrap_or(&edge.target_path_or_symbol)
                .trim();
            pick_preferred_symbol(sym, &edge.source_file, file_imports, graph)
        }
        _ => None,
    }
}

/// Resolves a TS/JS import specifier into a repository-relative file path.
pub fn resolve_module_specifier(
    source_file: &str,
    specifier: &str,
    graph: &CodeGraph,
    rules: &TsConfigRules,
) -> Option<String> {
    if specifier.starts_with('.') {
        let base_dir = source_file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let joined = normalize_join_rel(base_dir, specifier);
        return probe_ts_extensions(&joined, graph);
    }

    for (pattern, replacements) in &rules.paths {
        if let Some(captured) = match_wildcard(pattern, specifier) {
            for repl in replacements {
                let expanded = repl.replace('*', captured);
                let base = rules.base_url.as_deref().unwrap_or("");
                let joined = join_normalized(base, &expanded);
                if let Some(found) = probe_ts_extensions(&joined, graph) {
                    return Some(found);
                }
            }
        }
    }

    if let Some(base) = &rules.base_url {
        let joined = join_normalized(base, specifier);
        if let Some(found) = probe_ts_extensions(&joined, graph) {
            return Some(found);
        }
    }

    probe_ts_extensions(specifier, graph)
}

fn probe_ts_extensions(stem: &str, graph: &CodeGraph) -> Option<String> {
    let clean = stem.trim_end_matches('/');
    for ext in TS_EXTENSIONS {
        let candidate = format!("{clean}{ext}");
        if graph.file_hashes.contains_key(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn match_wildcard<'a>(pattern: &str, text: &'a str) -> Option<&'a str> {
    if let Some((prefix, suffix)) = pattern.split_once('*') {
        if text.starts_with(prefix)
            && text.ends_with(suffix)
            && text.len() >= prefix.len() + suffix.len()
        {
            return Some(&text[prefix.len()..text.len() - suffix.len()]);
        }
        None
    } else {
        (pattern == text).then_some("")
    }
}
