//! PHP/Laravel Tier 1 static analysis runner (PHPStan/Larastan -> Psalm -> `php -l`).

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::Result;
use anvil_core::compressor::compress_output;
use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::{resolve_local_or_path_bin, resolve_php_bin, resolve_php_vendor_script};
use anvil_core::proc_util::run_command_with_timeout;
use anvil_core::traits::CompressedOutput;

use super::typecheck_filter::filter_typecheck_output;

/// Runs PHPStan/Larastan, Psalm, or fallback `php -l` for impacted PHP files inside `work_dir`.
pub(super) fn run_php_typecheck(
    log_root: &Path,
    work_dir: &Path,
    config: &AnvilConfig,
    impacted_files: &[String],
) -> Result<CompressedOutput> {
    let php_files: Vec<&str> = impacted_files
        .iter()
        .filter(|f| f.ends_with(".php") && !f.ends_with(".blade.php"))
        .map(String::as_str)
        .collect();

    if let Some((mut cmd, label)) = build_phpstan_command(work_dir, &php_files) {
        return exec_and_filter(log_root, config, &mut cmd, &label, impacted_files);
    }
    if let Some((mut cmd, label)) = build_psalm_command(work_dir, &php_files) {
        return exec_and_filter(log_root, config, &mut cmd, &label, impacted_files);
    }
    if let Some(php_bin) = resolve_php_bin() {
        return run_php_lint_fallback(log_root, work_dir, config, &php_bin, &php_files);
    }

    Ok(CompressedOutput {
        tool: "phpstan".to_owned(),
        passed: true,
        exit_code: 0,
        summary: "SKIP (phpstan/psalm/php): no PHP static analyzer or php runtime found".to_owned(),
        lines: Vec::new(),
        total_errors: 0,
        truncated: false,
        raw_log_path: None,
    })
}

/// Builds a command invoking a PHP tool from `vendor/bin/<name>` (preferred with resolved `php`) or `PATH`.
pub(crate) fn build_php_tool_command(work_dir: &Path, tool_name: &str) -> Option<Command> {
    if let Some(script) = resolve_php_vendor_script(work_dir, tool_name)
        && let Some(php_bin) = resolve_php_bin()
    {
        let mut cmd = Command::new(php_bin);
        cmd.arg(script);
        cmd.env("COMPOSER_RUNTIME_BIN_DIR", work_dir.join("vendor").join("bin"));
        cmd.current_dir(work_dir);
        return Some(cmd);
    }
    if let Some(bin) = resolve_local_or_path_bin(work_dir, tool_name) {
        let mut cmd = Command::new(bin);
        cmd.current_dir(work_dir);
        prepend_php_dir_to_path(&mut cmd);
        return Some(cmd);
    }
    None
}

pub(crate) fn prepend_php_dir_to_path(cmd: &mut Command) {
    let Some(php_bin) = resolve_php_bin() else {
        return;
    };
    let Some(php_dir) = php_bin.parent() else {
        return;
    };
    let mut paths = vec![php_dir.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env("PATH", joined);
    }
}

fn build_phpstan_command(work_dir: &Path, php_files: &[&str]) -> Option<(Command, String)> {
    let mut cmd = build_php_tool_command(work_dir, "phpstan")?;
    cmd.args(["analyse", "--error-format=raw", "--no-progress"]);
    if !php_files.is_empty() {
        cmd.args(php_files);
    }
    Some((cmd, "phpstan analyse".to_owned()))
}

fn build_psalm_command(work_dir: &Path, php_files: &[&str]) -> Option<(Command, String)> {
    let mut cmd = build_php_tool_command(work_dir, "psalm")?;
    cmd.args(["--output-format=compact", "--no-progress"]);
    if !php_files.is_empty() {
        cmd.args(php_files);
    }
    Some((cmd, "psalm".to_owned()))
}

fn run_php_lint_fallback(
    log_root: &Path,
    work_dir: &Path,
    config: &AnvilConfig,
    php_bin: &Path,
    php_files: &[&str],
) -> Result<CompressedOutput> {
    if php_files.is_empty() {
        return Ok(CompressedOutput {
            tool: "php".to_owned(),
            passed: true,
            exit_code: 0,
            summary: "PASS (php -l): 0 modified PHP files to lint".to_owned(),
            lines: Vec::new(),
            total_errors: 0,
            truncated: false,
            raw_log_path: None,
        });
    }
    let timeout = Duration::from_secs(config.tier_budgets.tier1_secs.max(5));
    let mut combined = String::new();
    let mut exit_code = 0i32;
    for file in php_files {
        let mut cmd = Command::new(php_bin);
        cmd.args(["-l", file]).current_dir(work_dir);
        match run_command_with_timeout(&mut cmd, timeout) {
            Ok(res) if res.status_code != 0 => {
                exit_code = res.status_code;
                combined.push_str(&String::from_utf8_lossy(&res.stdout));
                combined.push_str(&String::from_utf8_lossy(&res.stderr));
                combined.push('\n');
            }
            Ok(_) => {}
            Err(err) => {
                exit_code = 1;
                combined.push_str(&format!("{file}:1: failed to run php -l: {err}\n"));
            }
        }
    }
    compress_output(
        Some(log_root),
        "php -l",
        exit_code,
        &combined,
        config.tier_budgets.compressor_max_lines,
        config.tier_budgets.compressor_max_tokens,
    )
}

fn exec_and_filter(
    log_root: &Path,
    config: &AnvilConfig,
    cmd: &mut Command,
    label: &str,
    impacted_files: &[String],
) -> Result<CompressedOutput> {
    let timeout = Duration::from_secs(config.tier_budgets.tier1_secs.max(5));
    let timed = match run_command_with_timeout(cmd, timeout) {
        Ok(out) => out,
        Err(err) => {
            return compress_output(
                Some(log_root),
                label,
                1,
                &format!("failed to execute `{label}`: {err}"),
                config.tier_budgets.compressor_max_lines,
                config.tier_budgets.compressor_max_tokens,
            );
        }
    };
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&timed.stdout),
        String::from_utf8_lossy(&timed.stderr)
    );
    filter_typecheck_output(
        Some(log_root),
        label,
        timed.status_code,
        &combined,
        impacted_files,
        config.tier_budgets.compressor_max_lines,
        config.tier_budgets.compressor_max_tokens,
    )
}
