//! `anvil prereqs`: detect what a machine needs for the project, check it, and write `init.sh`/`init.ps1`.

mod render;
mod script;

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_core::config::prereqs::PrereqSet;
use anvil_core::stack_detector;
use anvil_prereqs::{
    CheckOptions, Project, Report, SystemHost, check, detect, projects_with, restrict_to_subrepo,
};
use anyhow::Result;

use crate::cli::subcmds::PrereqsSubcmd;
use crate::commands::ExitCodeError;

/// Dispatches `anvil prereqs <sub>`.
pub fn execute(root: &Path, sub: PrereqsSubcmd) -> Result<()> {
    match sub {
        PrereqsSubcmd::Detect { write } => detect_cmd(root, write),
        PrereqsSubcmd::Check { json, subrepo } => check_cmd(root, json, subrepo.as_deref()),
        PrereqsSubcmd::Script { out } => script::write_scripts(root, out.as_deref()),
    }
}

/// The workspace root plus its subrepos: those in the config, else whatever stack detection finds.
pub(crate) fn projects_for(root: &Path, config: &AnvilConfig) -> Result<Vec<Project>> {
    let subrepos: Vec<(String, String)> = if config.subrepos.is_empty() {
        let detection = stack_detector::detect_workspace(root)?;
        detection
            .repos
            .into_iter()
            .filter(|r| r.rel_path != ".")
            .map(|r| (r.name, r.rel_path))
            .collect()
    } else {
        config.subrepos.iter().map(|s| (s.name.clone(), s.path.clone())).collect()
    };
    Ok(projects_with(&subrepos))
}

/// Detects the prerequisites of `root` from its files.
pub(crate) fn detect_set(root: &Path, config: &AnvilConfig) -> Result<(Vec<Project>, PrereqSet)> {
    let projects = projects_for(root, config)?;
    let set = detect(root, &projects);
    Ok((projects, set))
}

/// Runs the check on this machine: the stored set when there is one, else a fresh detection, with the
/// user's overrides applied either way.
pub(crate) fn run_check(
    root: &Path,
    config: &AnvilConfig,
    subrepo: Option<&str>,
) -> Result<Report> {
    let prerequisites = &config.prerequisites;
    let (projects, set) = if prerequisites.detected == PrereqSet::default() {
        let (projects, fresh) = detect_set(root, config)?;
        (projects, prerequisites.with_detected(fresh))
    } else {
        (projects_for(root, config)?, prerequisites.effective())
    };
    let (projects, set) = match subrepo {
        Some(name) => restrict_to_subrepo(&projects, &set, name)?,
        None => (projects, set),
    };
    Ok(check(&SystemHost, root, &projects, &set, &CheckOptions::default()))
}

/// Prints the check table; shared with `anvil doctor --prereqs`.
pub(crate) fn print_report(report: &Report) {
    render::print_report(report);
}

/// `12 ok, 1 missing, 1 wrong version, 2 warning(s)`.
pub(crate) fn counts(report: &Report) -> String {
    let s = report.summary;
    format!(
        "{} ok, {} missing, {} wrong version, {} warning(s)",
        s.ok, s.missing, s.wrong_version, s.warning
    )
}

fn detect_cmd(root: &Path, write: bool) -> Result<()> {
    let mut config = AnvilConfig::load_or_default(root)?;
    let (projects, set) = detect_set(root, &config)?;
    render::print_detected(&projects, &set, &config.prerequisites);
    if write {
        config.prerequisites.detected = set;
        config.save(root)?;
        println!("stored in {}", AnvilConfig::path(root).display());
    }
    Ok(())
}

fn check_cmd(root: &Path, json: bool, subrepo: Option<&str>) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    let report = run_check(root, &config, subrepo)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render::print_report(&report);
    }
    if report.ok { Ok(()) } else { Err(ExitCodeError::silent(1).into()) }
}
