//! `anvil docs` commands that write: init, new, scaffold, refresh, verify.

use std::path::Path;
use std::process::Command;

use anvil_core::config::AnvilConfig;
use anvil_docs::bundle::{self, Bundle};
use anvil_docs::concept::{self, Concept, scan_all};
use anvil_docs::create::{self, NewConcept};
use anvil_docs::drift::Drift;
use anvil_docs::refresh as refreshing;
use anvil_docs::scaffold;
use anyhow::{Result, bail};

use super::{load_graph, pick_bundle};

/// `anvil docs init [--project P]`.
pub fn init(root: &Path, config: &AnvilConfig, project: Option<&str>) -> Result<()> {
    for bundle in bundle::planned(root, config, project)? {
        let created = create::init_bundle(&bundle)?;
        if created.is_empty() {
            println!("docs: {} already initialised", bundle.repo_rel());
        } else {
            println!("docs: initialised {} ({} file(s))", bundle.repo_rel(), created.len());
        }
    }
    Ok(())
}

/// `anvil docs new <module> --paths <glob>...`.
pub fn new_concept(
    root: &Path,
    config: &AnvilConfig,
    project: Option<&str>,
    spec: &NewConcept,
) -> Result<()> {
    let bundle = pick_bundle(root, config, project)?;
    let graph = load_graph(root);
    let file = create::create_concept(root, &bundle, spec, graph.as_ref())?;
    let shown = file.strip_prefix(root).unwrap_or(&file).display();
    println!("docs: created {}", shown.to_string().replace('\\', "/"));
    if graph.is_none() {
        println!(
            "docs: no code graph yet; run `anvil graph build` then `anvil docs refresh` to fill the outline"
        );
    }
    Ok(())
}

/// `anvil docs scaffold [--project P] [--depth N]`.
pub fn scaffold(
    root: &Path,
    config: &AnvilConfig,
    project: Option<&str>,
    depth: Option<usize>,
) -> Result<()> {
    let Some(graph) = load_graph(root) else {
        bail!("no code graph found; run `anvil graph build` first");
    };
    for bundle in bundle::planned(root, config, project)? {
        let known: Vec<String> =
            concept::scan(root, &bundle, false).concepts.iter().map(Concept::key).collect();
        let mut created = 0usize;
        let plans = scaffold::plan(&graph, &bundle, depth);
        for plan in &plans {
            let key = format!("{}/{}", bundle.project, plan.module);
            if known.contains(&key) || bundle.dir.join(&plan.module).join("overview.md").exists() {
                continue;
            }
            create::create_concept(root, &bundle, &plan.to_concept(), Some(&graph))?;
            println!("docs: created {key} ({} files)", plan.files);
            created += 1;
        }
        println!(
            "docs: {}: {created} concept(s) created, {} module(s) already documented",
            bundle.project,
            plans.len() - created
        );
    }
    Ok(())
}

/// `anvil docs refresh <id>... | --stale`.
pub fn refresh(root: &Path, config: &AnvilConfig, ids: &[String], stale: bool) -> Result<()> {
    if ids.is_empty() && !stale {
        bail!("name the concepts to refresh, or pass --stale");
    }
    let bundles = bundle::existing(root, config);
    let concepts = scan_all(root, &bundles, true);
    let selected: Vec<&Concept> = if stale {
        let mut drift = Drift::new(root);
        concepts.iter().filter(|c| drift.check(c).is_stale()).collect()
    } else {
        ids.iter()
            .map(|id| concept::find(&concepts, id).ok_or_else(|| unknown(id, &concepts)))
            .collect::<Result<_>>()?
    };
    if selected.is_empty() {
        println!("docs: nothing to refresh");
        return Ok(());
    }
    let graph = load_graph(root);
    for concept in selected {
        let bundle = bundle_of(&bundles, concept)?;
        let done = refreshing::refresh_concept(root, bundle, concept, graph.as_ref())?;
        let outline =
            if done.outline_updated { "outline regenerated" } else { "no outline block updated" };
        let commit = done.synced_commit.as_deref().unwrap_or("none");
        println!("docs: refreshed {} ({outline}; synced to {commit})", done.key);
    }
    Ok(())
}

/// `anvil docs verify <id> [--by <name>]`: a human review; agents never run this.
pub fn verify(root: &Path, config: &AnvilConfig, id: &str, by: Option<&str>) -> Result<()> {
    let bundles = bundle::existing(root, config);
    let concepts = scan_all(root, &bundles, true);
    let concept = concept::find(&concepts, id).ok_or_else(|| unknown(id, &concepts))?;
    let name = by.map(str::to_owned).or_else(|| git_user_name(root)).unwrap_or_default();
    let stamp = refreshing::verify_concept(bundle_of(&bundles, concept)?, concept, &name)?;
    println!("docs: verified {} by {}", concept.key(), stamp.by);
    Ok(())
}

fn bundle_of<'a>(bundles: &'a [Bundle], concept: &Concept) -> Result<&'a Bundle> {
    match bundles.iter().find(|b| b.project == concept.project) {
        Some(bundle) => Ok(bundle),
        None => bail!("no bundle for project `{}`", concept.project),
    }
}

fn unknown(id: &str, concepts: &[Concept]) -> anyhow::Error {
    let known: Vec<String> = concepts.iter().map(Concept::key).collect();
    anyhow::anyhow!("unknown concept `{id}` (known: {})", known.join(", "))
}

fn git_user_name(root: &Path) -> Option<String> {
    let out =
        Command::new("git").arg("-C").arg(root).args(["config", "user.name"]).output().ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !name.is_empty()).then_some(name)
}
