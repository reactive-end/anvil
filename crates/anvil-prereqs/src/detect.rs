//! Detects what a project needs from the files it already has (manifests, lockfiles, version pins).

mod extras;
mod infra;
mod jvm;
mod langs;
mod node;
mod pins;
mod python;

use std::path::{Path, PathBuf};

use anvil_core::config::prereqs::{CustomPrereq, EnvVarPrereq, PathPrereq, PrereqSet, ToolPrereq};
use ignore::WalkBuilder;

use crate::envfile::read_text;
use crate::hints::install_hint;
use crate::probe::canonical;
use crate::project::Project;

/// Detects the prerequisites of every project. The workspace root always needs `git` and `anvil`.
pub fn detect(root: &Path, projects: &[Project]) -> PrereqSet {
    let nested: Vec<PathBuf> =
        projects.iter().filter(|p| p.name.is_some()).map(|p| p.dir(root)).collect();
    let mut all = PrereqSet::default();
    for project in projects {
        let mut scan = Scan::new(root, project, &nested);
        if project.name.is_none() {
            scan.tool("git", None, "anvil prereqs (always)".to_owned(), true);
            scan.tool("anvil", None, "anvil prereqs (always)".to_owned(), true);
        }
        pins::run(&mut scan);
        node::run(&mut scan);
        python::run(&mut scan);
        jvm::run(&mut scan);
        langs::run(&mut scan);
        infra::run(&mut scan);
        extras::run(&mut scan);
        merge(&mut all, scan.out);
    }
    all
}

fn merge(all: &mut PrereqSet, part: PrereqSet) {
    all.tools.extend(part.tools);
    all.env_files.extend(part.env_files);
    all.env_vars.extend(part.env_vars);
    all.paths.extend(part.paths);
    for server in part.mcp_servers {
        if !all.mcp_servers.contains(&server) {
            all.mcp_servers.push(server);
        }
    }
    all.custom.extend(part.custom);
}

/// Everything detectors share: where the project is and where its findings go.
pub(crate) struct Scan<'a> {
    pub project: &'a Project,
    pub dir: PathBuf,
    root: PathBuf,
    nested: &'a [PathBuf],
    pub out: PrereqSet,
}

impl<'a> Scan<'a> {
    fn new(root: &Path, project: &'a Project, nested: &'a [PathBuf]) -> Self {
        Self {
            project,
            dir: project.dir(root),
            root: root.to_path_buf(),
            nested,
            out: PrereqSet::default(),
        }
    }

    pub fn subrepo(&self) -> Option<String> {
        self.project.name.clone()
    }

    pub fn read(&self, rel: &str) -> Option<String> {
        read_text(&self.dir.join(rel))
    }

    /// True when a parent directory (up to the workspace root) holds a Cargo `[workspace]`.
    pub fn in_cargo_workspace(&self) -> bool {
        self.dir
            .ancestors()
            .skip(1)
            .take_while(|d| d.starts_with(&self.root))
            .any(|d| read_text(&d.join("Cargo.toml")).is_some_and(|t| t.contains("[workspace]")))
    }

    pub fn has(&self, rel: &str) -> bool {
        self.dir.join(rel).exists()
    }

    /// `orders-api/package.json engines.node`: the file relative to the workspace, plus a detail.
    pub fn src(&self, rel: &str, detail: &str) -> String {
        let file = self.project.rel_file(rel);
        if detail.is_empty() { file } else { format!("{file} {detail}") }
    }

    /// Adds a tool unless the same name and version is already there.
    pub fn tool(&mut self, name: &str, version: Option<String>, source: String, required: bool) {
        self.push_tool(name, version, source, required, None);
    }

    /// Like [`Scan::tool`] with a custom hint.
    pub fn tool_with_hint(
        &mut self,
        name: &str,
        version: Option<String>,
        source: String,
        hint: String,
    ) {
        self.push_tool(name, version, source, true, Some(hint));
    }

    fn push_tool(
        &mut self,
        name: &str,
        version: Option<String>,
        source: String,
        required: bool,
        hint: Option<String>,
    ) {
        let name = canonical(name);
        if self.out.tools.iter().any(|t| t.name == name && t.version == version) {
            return;
        }
        let hint = hint.unwrap_or_else(|| install_hint(&name, version.as_deref()));
        let subrepo = self.subrepo();
        self.out.tools.push(ToolPrereq { name, version, source, subrepo, required, hint });
    }

    pub fn env_var(&mut self, name: &str, source: String, required: bool, hint: &str) {
        let subrepo = self.subrepo();
        if self.out.env_vars.iter().any(|v| v.name == name) {
            return;
        }
        let hint = hint.to_owned();
        self.out.env_vars.push(EnvVarPrereq {
            name: name.to_owned(),
            source,
            subrepo,
            required,
            hint,
        });
    }

    pub fn path(&mut self, rel: &str, source: String, required: bool, hint: &str) {
        let subrepo = self.subrepo();
        let path = self.project.rel_file(rel);
        let hint = hint.to_owned();
        self.out.paths.push(PathPrereq { path, source, subrepo, required, hint });
    }

    pub fn custom(&mut self, name: &str, command: String, hint: &str, required: bool) {
        let subrepo = self.subrepo();
        let name = name.to_owned();
        let hint = hint.to_owned();
        self.out.custom.push(CustomPrereq { name, command, hint, subrepo, required });
    }

    /// Files under the project (at most `depth` levels down) whose name satisfies `keep`, skipping the
    /// directories of other projects and the usual build and dependency folders.
    pub fn find_files(&self, depth: usize, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
        let nested = self.nested.to_vec();
        let own = self.dir.clone();
        let walker = WalkBuilder::new(&self.dir)
            .max_depth(Some(depth))
            .filter_entry(move |e| {
                let name = e.file_name().to_string_lossy();
                let skip =
                    matches!(&*name, "node_modules" | "target" | "vendor" | "dist" | "build")
                        || nested.iter().any(|n| n != &own && n == e.path());
                !skip
            })
            .build();
        let mut found: Vec<PathBuf> = walker
            .flatten()
            .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
            .filter(|e| keep(&e.file_name().to_string_lossy()))
            .map(|e| e.into_path())
            .collect();
        found.sort();
        found
    }

    /// The workspace-relative path of `file` (forward slashes).
    pub fn rel_of(&self, file: &Path) -> String {
        let inside =
            file.strip_prefix(&self.dir).unwrap_or(file).to_string_lossy().replace('\\', "/");
        self.project.rel_file(&inside)
    }
}
