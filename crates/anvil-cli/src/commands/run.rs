//! `anvil run`: execute a command with token-compressed output and Circuit Breaker tracking.

use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use anvil_core::circuit_breaker::CircuitBreaker;
use anvil_core::compressor;
use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::resolve_local_or_path_bin;
use anvil_core::proc_util::{TimedOutput, run_command_with_timeout};
use anvil_core::stats::{StatsEvent, record_event};

use crate::commands::ExitCodeError;
use crate::ui;

/// Runs `cmd` in `root`, compresses output, updates Circuit Breaker, and preserves exit code.
pub fn execute(root: &Path, cmd: &[String]) -> Result<()> {
    let (program, args) = cmd.split_first().context("missing command after `anvil run --`")?;
    let config = AnvilConfig::load_or_default(root)?;
    let cmd_str = cmd.join(" ");
    let timeout = resolve_run_timeout(&config);

    let timed = run_process(root, program, args, cmd, timeout)?;
    let exit_code = timed.status_code;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&timed.stdout),
        String::from_utf8_lossy(&timed.stderr)
    );

    let compressed = compressor::compress_output(
        Some(root),
        &cmd_str,
        exit_code,
        &combined,
        config.tier_budgets.compressor_max_lines,
        config.tier_budgets.compressor_max_tokens,
    )?;

    let raw_bytes = combined.len();
    record_compressor_stats(root, raw_bytes, &compressed);

    let cb = CircuitBreaker::new(root, config.circuit_breaker_limit);
    if compressed.passed {
        let _ = cb.record_success();
        println!(
            "{} ({}): {}",
            ui::green("PASS"),
            ui::cyan(&compressed.tool),
            compressed.summary
        );
        return Ok(());
    }

    let failure_sample = if compressed.lines.is_empty() {
        compressed.summary.clone()
    } else {
        compressed.lines.join("\n")
    };
    let status = cb.record_failure(&compressed.tool, &failure_sample, &cmd_str)?;
    print_failure_report(&compressed, status.is_open(), status.consecutive_failures);
    Err(ExitCodeError::silent(exit_code).into())
}

fn resolve_run_timeout(config: &AnvilConfig) -> Duration {
    let secs = std::env::var("ANVIL_RUN_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or_else(|| config.tier_budgets.tier1_secs.max(120));
    Duration::from_secs(secs)
}

fn print_failure_report(
    compressed: &anvil_core::traits::CompressedOutput,
    circuit_open: bool,
    consecutive_failures: u32,
) {
    println!(
        "{} ({}, exit={}, errors={}): {}",
        ui::red("FAIL"),
        ui::cyan(&compressed.tool),
        compressed.exit_code,
        compressed.total_errors,
        compressed.summary
    );
    for line in &compressed.lines {
        println!("{line}");
    }
    if let Some(log_path) = &compressed.raw_log_path {
        println!("{} {}", ui::dim("raw log:"), ui::dim(&log_path.display().to_string()));
    }
    if circuit_open {
        eprintln!(
            "{}",
            ui::red(&format!(
                "CIRCUIT BREAKER OPEN ({consecutive_failures} identical failures). Inspect root cause and run `anvil circuit reset`."
            ))
        );
    }
}

fn record_compressor_stats(
    root: &Path,
    raw_bytes: usize,
    compressed: &anvil_core::traits::CompressedOutput,
) {
    let compact_bytes = if compressed.passed {
        compressed.tool.len() + compressed.summary.len() + 9
    } else {
        compressed.summary.len()
            + compressed
                .lines
                .iter()
                .map(|l| l.len() + 1)
                .sum::<usize>()
    };
    if raw_bytes > compact_bytes {
        let _ = record_event(
            root,
            StatsEvent::CompressorSaved {
                raw_bytes,
                compact_bytes,
            },
        );
    }
}

fn run_process(
    root: &Path,
    program: &str,
    args: &[String],
    full_cmd: &[String],
    timeout: Duration,
) -> Result<TimedOutput> {
    if let Some(resolved_bin) = resolve_local_or_path_bin(root, program) {
        let mut command = Command::new(&resolved_bin);
        command.args(args).current_dir(root);
        return run_command_with_timeout(&mut command, timeout)
            .with_context(|| format!("failed to execute `{}`", resolved_bin.display()));
    }

    let mut command = Command::new(program);
    command.args(args).current_dir(root);
    match run_command_with_timeout(&mut command, timeout) {
        Ok(out) => Ok(out),
        Err(err) if is_not_found_error(&err) => run_via_shell(root, full_cmd, timeout),
        Err(err) => bail!("failed to execute `{program}`: {err}"),
    }
}

fn is_not_found_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == ErrorKind::NotFound)
    })
}

fn run_via_shell(root: &Path, cmd: &[String], timeout: Duration) -> Result<TimedOutput> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let escaped = cmd
            .iter()
            .map(|arg| escape_windows_cmd_arg(arg))
            .collect::<Vec<_>>()
            .join(" ");
        let mut command = Command::new("cmd");
        command
            .args(["/D", "/S", "/C"])
            .raw_arg(format!("\"{escaped}\""))
            .current_dir(root);
        run_command_with_timeout(&mut command, timeout)
            .with_context(|| format!("failed to execute `{escaped}` via cmd /C"))
    }
    #[cfg(not(windows))]
    {
        let escaped = cmd
            .iter()
            .map(|arg| escape_posix_shell_arg(arg))
            .collect::<Vec<_>>()
            .join(" ");
        let mut command = Command::new("sh");
        command.args(["-c", &escaped]).current_dir(root);
        run_command_with_timeout(&mut command, timeout)
            .with_context(|| format!("failed to execute `{escaped}` via sh -c"))
    }
}

#[cfg(windows)]
fn escape_windows_cmd_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_owned();
    }
    let has_space_or_quote = arg.chars().any(|c| c.is_whitespace() || c == '"');
    if !has_space_or_quote {
        let mut out = String::with_capacity(arg.len() + 4);
        for ch in arg.chars() {
            if matches!(
                ch,
                '&' | '|' | '<' | '>' | '(' | ')' | '^' | '%' | '!' | ';' | ',' | '='
            ) {
                out.push('^');
            }
            out.push(ch);
        }
        return out;
    }

    let mut out = String::with_capacity(arg.len() + 8);
    out.push('"');
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2));
                backslashes = 0;
                out.push_str("\"\\^\"\"");
            }
            '%' => {
                out.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                out.push_str("\"^%\"");
            }
            _ => {
                if backslashes > 0 {
                    out.push_str(&"\\".repeat(backslashes));
                    backslashes = 0;
                }
                out.push(ch);
            }
        }
    }
    if backslashes > 0 {
        out.push_str(&"\\".repeat(backslashes * 2));
    }
    out.push('"');
    out
}

#[cfg(not(windows))]
fn escape_posix_shell_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_owned();
    }
    let needs_quotes = arg.chars().any(|c| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\'' | '&' | '|' | ';' | '<' | '>' | '(' | ')' | '$' | '^' | '%'
            )
    });
    if !needs_quotes {
        return arg.to_owned();
    }
    let escaped = arg.replace('\'', "'\\''");
    format!("'{escaped}'")
}
