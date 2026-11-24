//! TypeScript (`tsgo` / `tsc` / `vue-tsc`) incremental off-repo command builder and fallback runner.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::Result;
use anvil_core::compressor::compress_output;
use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::resolve_local_or_path_bin;
use anvil_core::proc_util::run_command_with_timeout;
use anvil_core::traits::CompressedOutput;

use super::typecheck_filter::filter_typecheck_output;

/// Selects `vue-tsc --noEmit` when Vue files or a Vue dependency are present and `vue-tsc` is
/// available; otherwise falls back to `build_ts_typecheck_command`.
pub fn build_vue_or_ts_typecheck_command(
    log_root: &Path,
    work_dir: &Path,
    impacted_files: &[String],
) -> Option<(PathBuf, Vec<String>, &'static str)> {
    let vue_applies = impacted_files.iter().any(|f| f.ends_with(".vue"))
        || fs::read_to_string(work_dir.join("package.json"))
            .is_ok_and(|s| s.contains("\"vue\""));
    if vue_applies
        && let Some(vue_tsc) = resolve_local_or_path_bin(work_dir, "vue-tsc")
    {
        return Some((vue_tsc, vec!["--noEmit".to_owned()], "vue-tsc"));
    }
    build_ts_typecheck_command(log_root, work_dir)
}

/// Builds a `tsgo`-first (falling back to `tsc`) `--noEmit --incremental` command targeting
/// `<log_root>/.harness/.state/tsbuildinfo-<work_hash>.tsbuildinfo` so the repo stays clean.
pub fn build_ts_typecheck_command(
    log_root: &Path,
    work_dir: &Path,
) -> Option<(PathBuf, Vec<String>, &'static str)> {
    let (bin, tool_name) = if let Some(tsgo) = resolve_local_or_path_bin(work_dir, "tsgo") {
        (tsgo, "tsgo")
    } else {
        (resolve_local_or_path_bin(work_dir, "tsc")?, "tsc")
    };

    let state_dir = log_root.join(".harness").join(".state");
    let _ = fs::create_dir_all(&state_dir);

    let hash_hex = blake3::hash(work_dir.to_string_lossy().as_bytes()).to_hex();
    let work_hash = &hash_hex[..16];
    let info_path = state_dir.join(format!("tsbuildinfo-{work_hash}.tsbuildinfo"));

    let args = vec![
        "--noEmit".to_owned(),
        "--incremental".to_owned(),
        "--tsBuildInfoFile".to_owned(),
        info_path.to_string_lossy().into_owned(),
    ];
    Some((bin, args, tool_name))
}

pub(super) fn run_ts_typecheck(
    log_root: &Path,
    work_dir: &Path,
    config: &AnvilConfig,
    impacted_files: &[String],
) -> Result<CompressedOutput> {
    let Some((bin, args, tool_name)) =
        build_vue_or_ts_typecheck_command(log_root, work_dir, impacted_files)
    else {
        return Ok(CompressedOutput {
            tool: "tsc".to_owned(),
            passed: true,
            exit_code: 0,
            summary: "SKIP (tsc): binary not found".to_owned(),
            lines: Vec::new(),
            total_errors: 0,
            truncated: false,
            raw_log_path: None,
        });
    };

    let timeout = Duration::from_secs(config.tier_budgets.tier1_secs.max(5));
    let label = format!("{tool_name} --noEmit");
    let (mut code, mut combined) = match exec_command(&bin, &args, work_dir, timeout) {
        Ok(res) => res,
        Err(err) => {
            return compress_output(
                Some(log_root),
                &label,
                1,
                &format!("failed to execute `{}`: {err}", bin.display()),
                config.tier_budgets.compressor_max_lines,
                config.tier_budgets.compressor_max_tokens,
            );
        }
    };

    if code != 0 && is_incremental_rejected(&combined) {
        let fallback_args = ["--noEmit".to_owned()];
        if let Ok(res) = exec_command(&bin, &fallback_args, work_dir, timeout) {
            code = res.0;
            combined = res.1;
        }
    }

    filter_typecheck_output(
        Some(log_root),
        &label,
        code,
        &combined,
        impacted_files,
        config.tier_budgets.compressor_max_lines,
        config.tier_budgets.compressor_max_tokens,
    )
}

fn exec_command(
    bin: &Path,
    args: &[String],
    work_dir: &Path,
    timeout: Duration,
) -> Result<(i32, String)> {
    let mut cmd = Command::new(bin);
    cmd.args(args).current_dir(work_dir);
    let timed = run_command_with_timeout(&mut cmd, timeout)?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&timed.stdout),
        String::from_utf8_lossy(&timed.stderr)
    );
    Ok((timed.status_code, combined))
}

fn is_incremental_rejected(output: &str) -> bool {
    output.contains("TS5069")
        || output.contains("TS5074")
        || output.contains("TS6305")
        || output.contains("Unknown compiler option")
        || output.contains("unknown compiler option")
}
