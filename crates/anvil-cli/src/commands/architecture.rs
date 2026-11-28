//! `anvil architecture`: record an architecture style, see the rules it generates, and check the repo.

mod check;
mod show;

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_core::config::architecture::ArchitectureStyle;
use anvil_core::config::architecture::detect::detect_style;
use anyhow::Result;

use crate::cli::subcmds::ArchitectureSubcmd;
use crate::ui;

/// Dispatches `anvil architecture <sub>`.
pub fn execute(root: &Path, sub: ArchitectureSubcmd) -> Result<()> {
    match sub {
        ArchitectureSubcmd::Show { json } => show::run(root, json),
        ArchitectureSubcmd::Set { style, subrepo, source_root } => {
            set_cmd(root, &style, subrepo.as_deref(), source_root.as_deref())
        }
        ArchitectureSubcmd::Detect => detect_cmd(root),
        ArchitectureSubcmd::Check { json } => check::run(root, json),
    }
}

fn set_cmd(
    root: &Path,
    style: &str,
    subrepo: Option<&str>,
    source_root: Option<&str>,
) -> Result<()> {
    let style: ArchitectureStyle = style.parse()?;
    let mut config = AnvilConfig::load_or_default(root)?;
    config.set_architecture(style, subrepo, source_root)?;
    config.save(root)?;

    let target = subrepo.unwrap_or("this repository");
    if style == ArchitectureStyle::None {
        println!("architecture rules removed for {target}");
        return Ok(());
    }
    let prefix = format!("arch:{style}:");
    let rules = config
        .architecture_rules()
        .iter()
        .filter(|r| r.name.starts_with(&prefix) && r.name.contains('@') == subrepo.is_some())
        .count();
    println!(
        "{} {style} ({}) recorded for {target}: {rules} rule(s) generated",
        ui::green("✓"),
        style.label()
    );
    ui::print_kv(
        "check",
        "anvil architecture check   (whole repo)  |  anvil check   (changed files)",
    );
    ui::print_kv("rules", "anvil architecture show");
    ui::print_kv(
        "agents",
        "anvil sync   (writes the Architecture line into CLAUDE.md, AGENTS.md, ...)",
    );
    Ok(())
}

fn detect_cmd(root: &Path) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    for target in config.architecture_all_targets() {
        let source = root.join(target.source_dir());
        let suggestion = detect_style(&source);
        println!("{}: {}", ui::bold(target.label()), suggestion.describe());
        let current = target.setting.style;
        if current != ArchitectureStyle::None {
            ui::print_kv("current", current.as_str());
        }
        if suggestion.style != ArchitectureStyle::None && suggestion.style != current {
            let flag = target.subrepo.map(|name| format!(" --subrepo {name}")).unwrap_or_default();
            ui::print_kv("apply", &format!("anvil architecture set {}{flag}", suggestion.style));
        }
    }
    Ok(())
}
