//! Terminal output of `anvil prereqs detect` and `anvil prereqs check`.

use anvil_core::config::prereqs::{PrereqSet, PrerequisitesConfig};
use anvil_prereqs::{Item, Kind, Project, Report, Status};

use crate::ui;

/// Compact table grouped by project: `✓` ok, `✗` required and failing, `!` warning.
pub(super) fn print_report(report: &Report) {
    let groups = report.groups();
    for group in &groups {
        if groups.len() > 1 {
            println!("{}", ui::bold(group));
        }
        for item in report.items.iter().filter(|i| i.group == *group) {
            print_item(item, groups.len() > 1);
        }
    }
    println!("{}", summary_line(report));
}

fn print_item(item: &Item, indent: bool) {
    let pad = if indent { "  " } else { "" };
    let mark = match item.status {
        Status::Ok => ui::green("✓"),
        Status::Warning => ui::yellow("!"),
        Status::Missing | Status::WrongVersion => ui::red("✗"),
    };
    println!("{pad}{mark} {:<18} {}", label(item), detail(item).trim_end());
    if item.status == Status::Ok {
        return;
    }
    if !item.source.is_empty() {
        println!("{pad}    {}", ui::dim(&format!("from {}", item.source)));
    }
    if let Some(hint) = &item.hint {
        println!("{pad}    {}", ui::dim(&format!("hint: {hint}")));
    }
}

fn label(item: &Item) -> String {
    match item.kind {
        Kind::Tool | Kind::Path => item.name.clone(),
        Kind::EnvFile => format!("env file {}", item.name),
        Kind::EnvVar => format!("env var {}", item.name),
        Kind::Mcp => format!("mcp {}", item.name),
        Kind::Secret => format!("secret {}", item.name),
        Kind::Custom => format!("check {}", item.name),
    }
}

fn detail(item: &Item) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(found) = &item.found {
        parts.push(found.clone());
    }
    match (&item.constraint, item.status) {
        (Some(c), Status::Ok) => parts.push(ui::dim(&format!("({c})"))),
        (Some(c), _) => parts.push(format!("needs {c}")),
        (None, _) => {}
    }
    if let Some(message) = &item.message {
        parts.push(message.clone());
    }
    parts.join("  ")
}

fn summary_line(report: &Report) -> String {
    let counts = super::counts(report);
    if report.ok {
        ui::green(&format!("✓ prereqs: {counts}"))
    } else {
        ui::red(&format!("✗ prereqs: {counts}; {} required item(s) to fix", report.failures()))
    }
}

/// What detection found, grouped by project.
pub(super) fn print_detected(projects: &[Project], set: &PrereqSet, config: &PrerequisitesConfig) {
    let mut names: Vec<Option<&str>> = projects.iter().map(|p| p.name.as_deref()).collect();
    names.dedup();
    for name in &names {
        if names.len() > 1 {
            println!("{}", ui::bold(name.unwrap_or("(root)")));
        }
        for tool in set.tools.iter().filter(|t| t.subrepo.as_deref() == *name) {
            let req = if tool.required { "" } else { " (optional)" };
            let version = tool.version.as_deref().unwrap_or("any");
            println!("  {:<16} {:<18} {}{req}", tool.name, version, ui::dim(&tool.source));
        }
        for pair in set.env_files.iter().filter(|e| e.subrepo.as_deref() == *name) {
            println!("  env file         {} <- {}", pair.file, pair.example);
        }
        for var in set.env_vars.iter().filter(|v| v.subrepo.as_deref() == *name) {
            println!("  env var          {}  {}", var.name, ui::dim(&var.source));
        }
        for path in set.paths.iter().filter(|p| p.subrepo.as_deref() == *name) {
            println!("  path             {}  {}", path.path, ui::dim(&path.source));
        }
        for check in set.custom.iter().filter(|c| c.subrepo.as_deref() == *name) {
            println!("  check            {}: {}", check.name, check.command);
        }
    }
    if !set.mcp_servers.is_empty() {
        println!("mcp servers: {}", set.mcp_servers.join(", "));
    }
    let overrides = config.extra_tools.len() + config.custom.len() + config.ignore.len();
    if overrides > 0 {
        println!(
            "{overrides} user override(s) in prerequisites are kept (extra_tools, custom, ignore)"
        );
    }
}
