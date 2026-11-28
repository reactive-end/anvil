//! Planning module concepts from the code graph.

use std::collections::BTreeMap;

use anvil_graph::model::{CodeGraph, NodeKind};
use rustc_hash::FxHashMap;

use crate::bundle::Bundle;
use crate::create::NewConcept;

/// Directories that group packages without being modules themselves.
const CONTAINERS: &[&str] = &["crates", "packages", "apps", "libs", "services"];
/// Conventional source roots, transparent when looking for modules.
const SOURCE_ROOTS: &[&str] = &["src", "lib", "app"];
/// Layer directories of NestJS-style modules.
const LAYERS: &[&str] = &["domain", "application", "infrastructure", "controller"];
/// Directories that hold no module of their own.
const NOISE: &[&str] = &["tests", "test", "__tests__", "fixtures", "benches", "examples", "e2e"];

/// A module the graph shows and a concept could document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModulePlan {
    /// Module path inside the bundle, e.g. `orders` or `anvil-core/config`.
    pub module: String,
    /// Repo-relative glob of the module code.
    pub glob: String,
    pub files: usize,
    pub exported: usize,
    /// Layer directories found inside the module with their file counts.
    pub layers: Vec<(String, usize)>,
}

impl ModulePlan {
    /// The concept this plan would create.
    pub fn to_concept(&self) -> NewConcept {
        let mut description =
            format!("{} source files, {} exported symbols.", self.files, self.exported);
        if !self.layers.is_empty() {
            let names: Vec<&str> = self.layers.iter().map(|(n, _)| n.as_str()).collect();
            description.push_str(&format!(" Layers: {}.", names.join(", ")));
        }
        NewConcept {
            module: self.module.clone(),
            paths: vec![self.glob.clone()],
            title: None,
            description: Some(description),
            responsibilities: self
                .layers
                .iter()
                .map(|(layer, n)| format!("`{layer}/` layer ({n} files)"))
                .collect(),
        }
    }
}

/// Position of a file inside its package: the names of the directories that count as levels and the
/// number of path segments each level spans.
struct Logical {
    names: Vec<String>,
    ends: Vec<usize>,
    packaged: bool,
}

fn logical(dirs: &[&str]) -> Logical {
    let mut out = Logical { names: Vec::new(), ends: Vec::new(), packaged: false };
    let mut next = 0;
    let first_is_package = dirs.first().is_some_and(|d| CONTAINERS.contains(d));
    let looks_packaged =
        dirs.len() > 1 && !SOURCE_ROOTS.contains(&dirs[0]) && SOURCE_ROOTS.contains(&dirs[1]);
    if first_is_package || looks_packaged {
        let package_at = usize::from(first_is_package);
        let Some(package) = dirs.get(package_at) else {
            return out;
        };
        out.names.push((*package).to_owned());
        out.ends.push(package_at + 1);
        out.packaged = true;
        next = package_at + 1;
    }
    if dirs.get(next).is_some_and(|d| SOURCE_ROOTS.contains(d)) {
        next += 1;
    }
    for (idx, dir) in dirs.iter().enumerate().skip(next) {
        out.names.push((*dir).to_owned());
        out.ends.push(idx + 1);
    }
    out
}

fn safe_segment(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') { c } else { '-' })
        .collect();
    mapped.trim_matches(['.', '-']).to_owned()
}

#[derive(Default)]
struct Group {
    dir: String,
    files: Vec<String>,
    shallow: bool,
}

/// Plans a concept for every module of `bundle` found in `graph`. `depth` counts levels below the
/// project root, where containers (`crates/`, `packages/`) and source roots (`src/`) are not levels;
/// by default packages are modules when the repository has any, else the folders under `src/`.
pub fn plan(graph: &CodeGraph, bundle: &Bundle, depth: Option<usize>) -> Vec<ModulePlan> {
    let prefix =
        if bundle.code_root.is_empty() { String::new() } else { format!("{}/", bundle.code_root) };
    let files: Vec<&str> = graph
        .file_hashes
        .keys()
        .filter(|f| !bundle.nested.iter().any(|n| f.starts_with(&format!("{n}/"))))
        .filter_map(|f| f.strip_prefix(prefix.as_str()))
        .collect();
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut any_packaged = false;
    let mut placed: Vec<(&str, Logical)> = Vec::new();
    for file in &files {
        let dirs: Vec<&str> = file.split('/').collect::<Vec<_>>();
        let dirs = &dirs[..dirs.len() - 1];
        let level = logical(dirs);
        any_packaged |= level.packaged;
        placed.push((file, level));
    }
    let wanted = depth.unwrap_or(if any_packaged { 2 } else { 1 }).max(1);
    for (file, level) in &placed {
        assign(&mut groups, file, level, wanted);
    }
    let shallow: Vec<String> =
        groups.iter().filter(|(_, g)| g.shallow).map(|(k, _)| k.clone()).collect();
    for key in shallow {
        if groups.keys().any(|other| other.starts_with(&format!("{key}/"))) {
            groups.remove(&key);
        }
    }
    let exported = exported_per_file(graph);
    groups.into_iter().map(|(module, group)| finish(bundle, module, &group, &exported)).collect()
}

fn assign(groups: &mut BTreeMap<String, Group>, file: &str, level: &Logical, wanted: usize) {
    let used = level.names.len().min(wanted);
    if used == 0 || NOISE.contains(&level.names[used - 1].as_str()) {
        return;
    }
    let key: Vec<String> = level.names[..used].iter().map(|n| safe_segment(n)).collect();
    if key.iter().any(String::is_empty) {
        return;
    }
    let segments: Vec<&str> = file.split('/').collect();
    let dir = segments[..level.ends[used - 1]].join("/");
    let group = groups.entry(key.join("/")).or_default();
    group.dir = dir;
    group.shallow = used < wanted;
    group.files.push(file.to_owned());
}

fn exported_per_file(graph: &CodeGraph) -> FxHashMap<&str, usize> {
    let mut counts: FxHashMap<&str, usize> = FxHashMap::default();
    for node in graph.nodes.iter().filter(|n| n.kind == NodeKind::Symbol && n.exported) {
        *counts.entry(node.file.as_str()).or_default() += 1;
    }
    counts
}

fn finish(
    bundle: &Bundle,
    module: String,
    group: &Group,
    exported: &FxHashMap<&str, usize>,
) -> ModulePlan {
    let root =
        if bundle.code_root.is_empty() { String::new() } else { format!("{}/", bundle.code_root) };
    let count: usize = group
        .files
        .iter()
        .map(|f| exported.get(format!("{root}{f}").as_str()).copied().unwrap_or(0))
        .sum();
    let mut layers: BTreeMap<String, usize> = BTreeMap::new();
    for file in &group.files {
        let below = file.strip_prefix(&format!("{}/", group.dir)).unwrap_or(file);
        if let Some((first, _)) = below.split_once('/')
            && LAYERS.contains(&first)
        {
            *layers.entry(first.to_owned()).or_default() += 1;
        }
    }
    ModulePlan {
        module,
        glob: format!("{root}{}/**", group.dir),
        files: group.files.len(),
        exported: count,
        layers: layers.into_iter().collect(),
    }
}
