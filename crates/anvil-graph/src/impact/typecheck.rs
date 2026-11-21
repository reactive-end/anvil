//! Filtered native compiler/typechecker invocation compressed via `anvil_core::compressor`.

use std::path::Path;
use std::process::Command;

use anyhow::Result;
use anvil_core::compressor::compress_output;
use anvil_core::config::{AnvilConfig, StackKind};
use anvil_core::fs_util::resolve_local_or_path_bin;
use anvil_core::stack_detector::detect_workspace;
use anvil_core::traits::CompressedOutput;

/// Runs native compiler/typechecker filtered to `impacted_files` and compresses output via `anvil-core`.
pub fn run_impacted_typecheck(
    repo_root: &Path,
    config: &AnvilConfig,
    impacted_files: &[String],
) -> Result<Vec<CompressedOutput>> {
    let stacks = resolve_stacks(repo_root, config);
    let mut results = Vec::new();
    if stacks.iter().any(is_ts_stack) {
        results.push(run_tool_and_filter(
            repo_root,
            config,
            "tsc",
            "tsc --noEmit",
            &["--noEmit"],
            impacted_files,
        )?);
    }
    if stacks.contains(&StackKind::Python) {
        run_ext_typecheck(
            repo_root,
            config,
            impacted_files,
            ".py",
            "pyright",
            &[],
            &mut results,
        )?;
    }
    if stacks.contains(&StackKind::FlutterDart) {
        run_ext_typecheck(
            repo_root,
            config,
            impacted_files,
            ".dart",
            "dart",
            &["analyze"],
            &mut results,
        )?;
    }
    Ok(results)
}

fn resolve_stacks(repo_root: &Path, config: &AnvilConfig) -> Vec<StackKind> {
    if !config.stacks.is_empty() {
        return config.stacks.clone();
    }
    detect_workspace(repo_root)
        .map(|d| d.repos.into_iter().flat_map(|r| r.stacks).collect())
        .unwrap_or_default()
}

fn is_ts_stack(stack: &StackKind) -> bool {
    matches!(
        stack,
        StackKind::TypeScript
            | StackKind::React
            | StackKind::Vite
            | StackKind::NextJs
            | StackKind::Astro
            | StackKind::Angular
            | StackKind::NestJs
            | StackKind::ExpoReactNative
    )
}

fn run_ext_typecheck(
    repo_root: &Path,
    config: &AnvilConfig,
    impacted_files: &[String],
    ext: &str,
    bin: &str,
    base_args: &[&str],
    results: &mut Vec<CompressedOutput>,
) -> Result<()> {
    let matching: Vec<String> = impacted_files
        .iter()
        .filter(|f| f.ends_with(ext))
        .cloned()
        .collect();
    if matching.is_empty() {
        return Ok(());
    }
    let mut args: Vec<&str> = base_args.to_vec();
    args.extend(matching.iter().map(String::as_str));
    let label = if base_args.is_empty() {
        bin.to_owned()
    } else {
        format!("{bin} {}", base_args.join(" "))
    };
    results.push(run_tool_and_filter(
        repo_root,
        config,
        bin,
        &label,
        &args,
        impacted_files,
    )?);
    Ok(())
}

fn run_tool_and_filter(
    repo_root: &Path,
    config: &AnvilConfig,
    bin_name: &str,
    label: &str,
    args: &[&str],
    impacted_files: &[String],
) -> Result<CompressedOutput> {
    let Some(bin_path) = resolve_local_or_path_bin(repo_root, bin_name) else {
        return Ok(CompressedOutput {
            tool: bin_name.to_owned(),
            passed: true,
            exit_code: 0,
            summary: format!("SKIP ({bin_name}): binary not found"),
            lines: Vec::new(),
            total_errors: 0,
            truncated: false,
            raw_log_path: None,
        });
    };

    let output = match Command::new(&bin_path).args(args).current_dir(repo_root).output() {
        Ok(out) => out,
        Err(err) => {
            return compress_output(
                Some(repo_root),
                label,
                1,
                &format!("failed to execute `{}`: {err}", bin_path.display()),
                config.tier_budgets.compressor_max_lines,
                config.tier_budgets.compressor_max_tokens,
            );
        }
    };

    let code = output.status.code().unwrap_or(1);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    filter_typecheck_output(
        Some(repo_root),
        label,
        code,
        &combined,
        impacted_files,
        config.tier_budgets.compressor_max_lines,
        config.tier_budgets.compressor_max_tokens,
    )
}

/// Filters compiler/typechecker output to `impacted_files` without leaking unrelated errors.
pub fn filter_typecheck_output(
    repo_root: Option<&Path>,
    label: &str,
    exit_code: i32,
    raw_output: &str,
    impacted_files: &[String],
    max_lines: usize,
    max_tokens: usize,
) -> Result<CompressedOutput> {
    if impacted_files.is_empty() {
        return compress_output(repo_root, label, exit_code, raw_output, max_lines, max_tokens);
    }

    let patterns: Vec<(String, String)> = impacted_files
        .iter()
        .map(|f| {
            let clean = f.trim_start_matches("./").replace('\\', "/");
            let win = clean.replace('/', "\\");
            (clean, win)
        })
        .collect();

    let mut kept = Vec::new();
    let mut suppressed_errors = 0usize;
    let mut in_kept_diag = false;

    for line in raw_output.lines() {
        let is_indented = line.starts_with(char::is_whitespace);
        let matches_impacted = patterns
            .iter()
            .any(|(fwd, bck)| line.contains(fwd.as_str()) || line.contains(bck.as_str()));

        if matches_impacted {
            kept.push(line);
            in_kept_diag = true;
        } else if is_indented && in_kept_diag {
            kept.push(line);
        } else {
            in_kept_diag = false;
            if looks_like_error_line(line) {
                suppressed_errors += 1;
            }
        }
    }

    if kept.is_empty() {
        if exit_code == 0 {
            return compress_output(repo_root, label, 0, "", max_lines, max_tokens);
        }
        let tool = label.split_whitespace().next().unwrap_or(label).to_owned();
        let count = suppressed_errors.max(1);
        return Ok(CompressedOutput {
            tool: tool.clone(),
            passed: true,
            exit_code: 0,
            summary: format!(
                "PASS ({tool}): 0 errors in modified/impacted files ({count} unrelated error(s) suppressed)"
            ),
            lines: Vec::new(),
            total_errors: 0,
            truncated: false,
            raw_log_path: None,
        });
    }

    let filtered = kept.join("\n");
    let effective_code = if exit_code == 0 { 1 } else { exit_code };
    compress_output(
        repo_root,
        label,
        effective_code,
        &filtered,
        max_lines,
        max_tokens,
    )
}

fn looks_like_error_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && !trimmed.starts_with(char::is_whitespace)
        && (trimmed.contains("error TS")
            || trimmed.contains(": error")
            || trimmed.contains(" - error")
            || trimmed.starts_with("error - "))
}
