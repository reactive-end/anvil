//! `anvil docs`: living project documentation in the Open Knowledge Format.

mod lifecycle;
mod report;

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_docs::bundle::{self, Bundle};
use anvil_graph::model::CodeGraph;
use anvil_graph::store;
use anyhow::{Result, bail};

use crate::cli::subcmds::DocsSubcmd;

/// Executes `anvil docs init|new|scaffold|list|check|refresh|verify|search`.
pub fn execute(root: &Path, sub: DocsSubcmd) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    match sub {
        DocsSubcmd::Init { project } => lifecycle::init(root, &config, project.as_deref()),
        DocsSubcmd::New { module, paths, project, title, description } => {
            let spec = anvil_docs::create::NewConcept {
                module,
                paths,
                title,
                description,
                responsibilities: Vec::new(),
            };
            lifecycle::new_concept(root, &config, project.as_deref(), &spec)
        }
        DocsSubcmd::Scaffold { project, depth } => {
            lifecycle::scaffold(root, &config, project.as_deref(), depth)
        }
        DocsSubcmd::List { json } => report::list(root, &config, json),
        DocsSubcmd::Check { strict, json } => report::check(root, &config, strict, json),
        DocsSubcmd::Refresh { ids, stale } => lifecycle::refresh(root, &config, &ids, stale),
        DocsSubcmd::Verify { id, by } => lifecycle::verify(root, &config, &id, by.as_deref()),
        DocsSubcmd::Search { query, limit, json } => {
            report::search(root, &config, &query, limit, json)
        }
    }
}

/// The code graph, when one was built.
fn load_graph(root: &Path) -> Option<CodeGraph> {
    let path = store::default_bin_path(root);
    path.is_file().then(|| store::load_binary(&path).ok()).flatten()
}

/// The single bundle a command targets: the only planned one, or the one named by `--project`.
fn pick_bundle(root: &Path, config: &AnvilConfig, project: Option<&str>) -> Result<Bundle> {
    let mut planned = bundle::planned(root, config, project)?;
    if planned.len() > 1 {
        let names: Vec<&str> = planned.iter().map(|b| b.project.as_str()).collect();
        bail!("several projects found; pass --project <{}>", names.join("|"));
    }
    Ok(planned.remove(0))
}
