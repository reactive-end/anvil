//! `anvil sonar`: opt-in local SonarQube status, New Code issues, and scanner runner.

use std::path::Path;
use std::process::Command;

use anvil_core::compressor::compress_output;
use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::resolve_sonar_bin;
use anvil_core::git;
use anvil_core::proc_util::run_command_with_timeout;
use anvil_core::sonar::{fetch_new_code_issues, fetch_quality_gate, parse_sonar_properties};
use anyhow::{Context, Result, bail};

use crate::cli::subcmds::SonarSubcmd;
use crate::commands::ExitCodeError;
use crate::ui;

/// Executes `anvil sonar <status|issues|analyze>`.
pub fn execute(root: &Path, sub: SonarSubcmd) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    match sub {
        SonarSubcmd::Status { host, project, branch } => {
            cmd_status(root, &config, host.as_deref(), project.as_deref(), branch.as_deref())
        }
        SonarSubcmd::Issues { host, project, branch, limit } => {
            cmd_issues(root, &config, host.as_deref(), project.as_deref(), branch.as_deref(), limit)
        }
        SonarSubcmd::Analyze { args } => cmd_analyze(root, &config, &args),
    }
}

/// Evaluates the configured local SonarQube Quality Gate during opt-in Tier 2 checks.
/// Returns `true` when the Quality Gate explicitly reports `ERROR`.
pub(crate) fn run_tier2_quality_gate(root: &Path, config: &AnvilConfig) -> bool {
    let (host, project, token) = resolve_sonar_endpoint(root, config, None, None);
    let (Some(host_url), Some(project_key)) = (host, project) else {
        println!(
            "  {} sonar enabled, but host/projectKey missing; configure `.harness/config.json` or `sonar-project.properties`",
            ui::yellow("[Tier 2]")
        );
        return false;
    };
    let branch = git::current_branch(root).ok();
    match fetch_quality_gate(&host_url, &project_key, branch.as_deref(), token.as_deref()) {
        Ok(report) if report.status == "ERROR" => {
            println!(
                "  {} {}",
                ui::cyan("[Tier 2]"),
                ui::red(&format!("sonar quality gate: ERROR ({project_key})"))
            );
            for c in report.conditions.iter().filter(|c| c.status == "ERROR") {
                println!(
                    "    - {} actual={} threshold={} ({})",
                    c.metric_key, c.actual_value, c.error_threshold, c.comparator
                );
            }
            true
        }
        Ok(report) => {
            println!(
                "  {} {}",
                ui::cyan("[Tier 2]"),
                ui::green(&format!("sonar quality gate: {} ({project_key})", report.status))
            );
            false
        }
        Err(err) => {
            println!(
                "  {} sonar quality gate skipped (local server unreachable: {err:#})",
                ui::yellow("[Tier 2]")
            );
            false
        }
    }
}

fn cmd_status(
    root: &Path,
    config: &AnvilConfig,
    host_override: Option<&str>,
    project_override: Option<&str>,
    branch_override: Option<&str>,
) -> Result<()> {
    let (host_url, project_key, token) =
        require_endpoint(root, config, host_override, project_override)?;
    let branch = branch_override.map(str::to_owned).or_else(|| git::current_branch(root).ok());
    let report = fetch_quality_gate(&host_url, &project_key, branch.as_deref(), token.as_deref())?;

    println!(
        "{} project={} status={}",
        ui::accent("anvil sonar status:"),
        ui::cyan(&project_key),
        if report.status == "OK" { ui::green(&report.status) } else { ui::red(&report.status) }
    );
    for cond in &report.conditions {
        println!(
            "  - [{}] {}: actual={} (threshold={} {})",
            cond.status, cond.metric_key, cond.actual_value, cond.comparator, cond.error_threshold
        );
    }
    if report.status == "ERROR" {
        return Err(ExitCodeError::silent(1).into());
    }
    Ok(())
}

fn cmd_issues(
    root: &Path,
    config: &AnvilConfig,
    host_override: Option<&str>,
    project_override: Option<&str>,
    branch_override: Option<&str>,
    limit: usize,
) -> Result<()> {
    let (host_url, project_key, token) =
        require_endpoint(root, config, host_override, project_override)?;
    let branch = branch_override.map(str::to_owned).or_else(|| git::current_branch(root).ok());
    let issues =
        fetch_new_code_issues(&host_url, &project_key, branch.as_deref(), token.as_deref(), limit)?;

    println!(
        "{} project={} new_code_issues={}",
        ui::accent("anvil sonar issues:"),
        ui::cyan(&project_key),
        issues.len()
    );
    for issue in &issues {
        println!(
            "  - [{}] {} {}:{} — {}",
            issue.severity, issue.rule, issue.component, issue.line, issue.message
        );
    }
    Ok(())
}

fn cmd_analyze(root: &Path, config: &AnvilConfig, extra_args: &[String]) -> Result<()> {
    let Some(bin) = resolve_sonar_bin() else {
        bail!(
            "SonarQube scanner binary (`sonar` / `sonar-scanner`) not found on PATH or SONAR_SCANNER_BIN"
        );
    };
    let (host, project, token) = resolve_sonar_endpoint(root, config, None, None);
    let mut cmd = Command::new(&bin);
    cmd.current_dir(root).args(extra_args);
    if let Some(ref url) = host
        && !extra_args.iter().any(|a| a.contains("sonar.host.url"))
    {
        cmd.arg(format!("-Dsonar.host.url={url}"));
    }
    if let Some(ref key) = project
        && !extra_args.iter().any(|a| a.contains("sonar.projectKey"))
    {
        cmd.arg(format!("-Dsonar.projectKey={key}"));
    }
    if let Some(ref tok) = token {
        cmd.env("SONAR_TOKEN", tok);
    }

    let timeout = config.tier_budgets.tier1_timeout();
    let timed = run_command_with_timeout(&mut cmd, timeout)
        .with_context(|| format!("failed to execute {}", bin.display()))?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&timed.stdout),
        String::from_utf8_lossy(&timed.stderr)
    );
    let exit_code = timed.status_code;
    let compressed = compress_output(
        Some(root),
        "sonar-scanner",
        exit_code,
        &combined,
        config.tier_budgets.compressor_max_lines,
        config.tier_budgets.compressor_max_tokens,
    )?;
    println!("{}", compressed.summary);
    for line in &compressed.lines {
        println!("  {line}");
    }
    if exit_code != 0 {
        return Err(ExitCodeError::silent(exit_code).into());
    }
    Ok(())
}

pub(crate) fn resolve_sonar_endpoint(
    root: &Path,
    config: &AnvilConfig,
    host_override: Option<&str>,
    project_override: Option<&str>,
) -> (Option<String>, Option<String>, Option<String>) {
    let props = parse_sonar_properties(root);
    let host = host_override
        .map(str::to_owned)
        .or_else(|| config.sonar.host_url.clone())
        .or_else(|| props.as_ref().and_then(|p| p.host_url.clone()))
        .or_else(|| std::env::var("SONAR_HOST_URL").ok().filter(|s| !s.is_empty()));
    let project = project_override
        .map(str::to_owned)
        .or_else(|| config.sonar.project_key.clone())
        .or_else(|| props.and_then(|p| p.project_key));
    let token = std::env::var(&config.sonar.token_env).ok().filter(|s| !s.trim().is_empty());
    (host, project, token)
}

fn require_endpoint(
    root: &Path,
    config: &AnvilConfig,
    host_override: Option<&str>,
    project_override: Option<&str>,
) -> Result<(String, String, Option<String>)> {
    let (host, project, token) =
        resolve_sonar_endpoint(root, config, host_override, project_override);
    let host_url = host.unwrap_or_else(|| "http://localhost:9000".to_owned());
    let project_key = project.ok_or_else(|| {
        anyhow::anyhow!(
            "missing SonarQube project key (set `--project`, `.harness/config.json`, or `sonar-project.properties`)"
        )
    })?;
    Ok((host_url, project_key, token))
}
