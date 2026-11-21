//! `anvil run`: execute a command with token-compressed output and Circuit Breaker tracking.

use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};
use anvil_core::circuit_breaker::CircuitBreaker;
use anvil_core::compressor;
use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::resolve_local_or_path_bin;

use crate::commands::ExitCodeError;

/// Runs `cmd` in `root`, compresses output, updates Circuit Breaker, and preserves exit code.
pub fn execute(root: &Path, cmd: &[String]) -> Result<()> {
    let (program, args) = cmd.split_first().context("missing command after `anvil run --`")?;
    let config = AnvilConfig::load_or_default(root)?;
    let cmd_str = cmd.join(" ");

    let output = run_process(root, program, args, cmd)?;
    let exit_code = output.status.code().unwrap_or(1);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let compressed = compressor::compress_output(
        Some(root),
        &cmd_str,
        exit_code,
        &combined,
        config.tier_budgets.compressor_max_lines,
        config.tier_budgets.compressor_max_tokens,
    )?;

    let cb = CircuitBreaker::new(root, config.circuit_breaker_limit);
    if compressed.passed {
        let _ = cb.record_success();
        println!("PASS ({}): {}", compressed.tool, compressed.summary);
        return Ok(());
    }

    let failure_sample = if compressed.lines.is_empty() {
        compressed.summary.clone()
    } else {
        compressed.lines.join("\n")
    };
    let status = cb.record_failure(&compressed.tool, &failure_sample, &cmd_str)?;

    println!(
        "FAIL ({}, exit={}, errors={}): {}",
        compressed.tool, compressed.exit_code, compressed.total_errors, compressed.summary
    );
    for line in &compressed.lines {
        println!("{line}");
    }
    if let Some(log_path) = &compressed.raw_log_path {
        println!("raw log: {}", log_path.display());
    }
    if status.is_open() {
        eprintln!(
            "CIRCUIT BREAKER OPEN ({} identical failures). Inspect root cause and run `anvil circuit reset`.",
            status.consecutive_failures
        );
    }

    Err(ExitCodeError::silent(exit_code).into())
}

fn run_process(root: &Path, program: &str, args: &[String], full_cmd: &[String]) -> Result<Output> {
    if let Some(resolved_bin) = resolve_local_or_path_bin(root, program) {
        return Command::new(&resolved_bin)
            .args(args)
            .current_dir(root)
            .output()
            .with_context(|| format!("failed to execute `{}`", resolved_bin.display()));
    }

    match Command::new(program).args(args).current_dir(root).output() {
        Ok(out) => Ok(out),
        Err(err) if err.kind() == ErrorKind::NotFound => run_via_shell(root, full_cmd),
        Err(err) => bail!("failed to execute `{program}`: {err}"),
    }
}

fn run_via_shell(root: &Path, cmd: &[String]) -> Result<Output> {
    let escaped = escape_shell_command(cmd);
    if cfg!(windows) {
        Command::new("cmd")
            .args(["/C", &escaped])
            .current_dir(root)
            .output()
            .with_context(|| format!("failed to execute `{escaped}` via cmd /C"))
    } else {
        Command::new("sh")
            .args(["-c", &escaped])
            .current_dir(root)
            .output()
            .with_context(|| format!("failed to execute `{escaped}` via sh -c"))
    }
}

fn escape_shell_command(cmd: &[String]) -> String {
    cmd.iter()
        .map(|arg| escape_shell_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn escape_shell_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_owned();
    }
    let needs_quotes = arg.chars().any(|c| {
        c.is_whitespace() || matches!(c, '"' | '\'' | '&' | '|' | ';' | '<' | '>' | '(' | ')' | '$')
    });
    if !needs_quotes {
        return arg.to_owned();
    }
    if cfg!(windows) {
        let escaped = arg.replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        let escaped = arg.replace('\'', "'\\''");
        format!("'{escaped}'")
    }
}
