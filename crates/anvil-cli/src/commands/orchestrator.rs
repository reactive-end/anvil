//! `anvil orchestrator`: pair a planning model with an execution model for Claude Code.

use std::path::Path;

use anyhow::Result;
use anvil_adapters::claude_code::agents;
use anvil_core::config::AnvilConfig;
use anvil_core::config::orchestrator::{
    EffortLevel, OrchestratorConfig, RoleModel, validate_model,
};
use anvil_core::orchestrator_mode;

use crate::cli::subcmds::OrchestratorSubcmd;
use crate::commands::ExitCodeError;
use crate::ui;
use crate::ui::prompt::{self, PromptOutcome, SelectOption};

const MODEL_CHOICES: &[(&str, &str)] = &[
    ("fable", "latest Fable: most capable"),
    ("opus", "latest Opus: strong planning and review"),
    ("sonnet", "latest Sonnet: balanced"),
    ("haiku", "latest Haiku: fastest and cheapest"),
];

/// Executes `anvil orchestrator <subcommand>`.
pub fn execute(root: &Path, sub: OrchestratorSubcmd) -> Result<()> {
    match sub {
        OrchestratorSubcmd::Setup {
            model,
            effort,
            executor_model,
            executor_effort,
        } => cmd_setup(
            root,
            [model.as_deref(), effort.as_deref()],
            [executor_model.as_deref(), executor_effort.as_deref()],
        ),
        OrchestratorSubcmd::Status { json } => cmd_status(root, json),
        OrchestratorSubcmd::On { default_agent } => cmd_on(root, default_agent),
        OrchestratorSubcmd::Off => cmd_off(root),
    }
}

fn cmd_setup(
    root: &Path,
    orchestrator: [Option<&str>; 2],
    executor: [Option<&str>; 2],
) -> Result<()> {
    let mut config = AnvilConfig::load(root)?;
    let current = config.orchestrator.clone().unwrap_or_default();
    let next = OrchestratorConfig {
        orchestrator: resolve_role("Orchestrator", orchestrator, &current.orchestrator)?,
        executor: resolve_role("Executor", executor, &current.executor)?,
        executor_tools: current.executor_tools.clone(),
    };
    config.orchestrator = Some(next.clone());
    config.save(root)?;
    for path in agents::sync_agents(root, &next)? {
        println!("wrote {}", path.display());
    }
    print_pair(&next);
    println!(
        "Start a session with `claude --agent {}`, type `/orchestrate <task>` in any session, or run `anvil orchestrator on`.",
        agents::ORCHESTRATOR_AGENT
    );
    Ok(())
}

fn cmd_status(root: &Path, json: bool) -> Result<()> {
    let configured = AnvilConfig::load_or_default(root)?.orchestrator;
    let enabled = orchestrator_mode::is_enabled(root);
    if json {
        let pair = configured.clone().unwrap_or_default();
        let report = serde_json::json!({
            "initialized": AnvilConfig::path(root).is_file(),
            "configured": configured.is_some(),
            "enabled": enabled,
            "orchestrator": pair.orchestrator,
            "executor": pair.executor,
        });
        println!("{report}");
        return Ok(());
    }
    let Some(pair) = configured else {
        println!("orchestrator: not configured (run `anvil orchestrator setup`)");
        return Ok(());
    };
    print_pair(&pair);
    ui::print_kv("mode:", if enabled { "on" } else { "off" });
    let state = if agents::is_default_agent(root) { "on" } else { "off" };
    ui::print_kv("default_agent:", state);
    Ok(())
}

fn cmd_on(root: &Path, default_agent: bool) -> Result<()> {
    let mut config = AnvilConfig::load(root)?;
    let pair = config.orchestrator.clone().unwrap_or_default();
    if config.orchestrator.is_none() {
        config.orchestrator = Some(pair.clone());
        config.save(root)?;
    }
    agents::sync_agents(root, &pair)?;
    if default_agent {
        agents::set_default_agent(root, true)?;
    }
    orchestrator_mode::set_enabled(root, true)?;
    print_pair(&pair);
    println!("orchestrator mode on: the main thread only judges; `anvil-executor` writes within its SCOPE");
    Ok(())
}

fn cmd_off(root: &Path) -> Result<()> {
    agents::set_default_agent(root, false)?;
    orchestrator_mode::set_enabled(root, false)?;
    println!("orchestrator mode off");
    Ok(())
}

fn print_pair(pair: &OrchestratorConfig) {
    for (label, role) in [("orchestrator:", &pair.orchestrator), ("executor:", &pair.executor)] {
        ui::print_kv(label, &format!("{} ({} effort)", role.model, role.effort.as_str()));
    }
}

fn resolve_role(label: &str, flags: [Option<&str>; 2], current: &RoleModel) -> Result<RoleModel> {
    let [model, effort] = flags;
    let interactive = model.is_none() && effort.is_none() && prompt::is_interactive_tui();
    Ok(RoleModel {
        model: match model {
            Some(raw) => validate_model(raw)?,
            None if interactive => pick_model(label, &current.model)?,
            None => current.model.clone(),
        },
        effort: match effort {
            Some(raw) => EffortLevel::parse(raw)?,
            None if interactive => pick_effort(label, current.effort)?,
            None => current.effort,
        },
    })
}

fn pick_model(label: &str, current: &str) -> Result<String> {
    let mut names: Vec<&str> = MODEL_CHOICES.iter().map(|(name, _)| *name).collect();
    let mut options: Vec<SelectOption> = MODEL_CHOICES
        .iter()
        .map(|(name, hint)| SelectOption::new(*name, *hint))
        .collect();
    if !names.contains(&current) {
        names.push(current);
        options.push(SelectOption::new(current, "current"));
    }
    let default_idx = names.iter().position(|name| *name == current).unwrap_or(0);
    let idx = pick(&format!("{label} model"), &options, default_idx)?;
    Ok(names[idx].to_owned())
}

fn pick_effort(label: &str, current: EffortLevel) -> Result<EffortLevel> {
    let options: Vec<SelectOption> = EffortLevel::ALL
        .iter()
        .map(|level| SelectOption::new(level.as_str(), ""))
        .collect();
    let default_idx = EffortLevel::ALL.iter().position(|l| *l == current).unwrap_or(0);
    Ok(EffortLevel::ALL[pick(&format!("{label} effort"), &options, default_idx)?])
}

fn pick(title: &str, options: &[SelectOption], default_idx: usize) -> Result<usize> {
    match prompt::select(title, title, None, options, default_idx)? {
        PromptOutcome::Confirmed(idx) => Ok(idx),
        PromptOutcome::Cancelled => Err(ExitCodeError::silent(130).into()),
    }
}
