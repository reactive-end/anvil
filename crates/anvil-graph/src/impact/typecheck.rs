//! Filtered native compiler/typechecker invocation compressed via `anvil_core::compressor`.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use anvil_core::compressor::compress_output;
use anvil_core::config::{AnvilConfig, StackKind};
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
        results.push(run_and_filter(
            repo_root,
            config,
            "npx tsc --noEmit",
            &["tsc", "--noEmit"],
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
            &["pyright"],
            &mut results,
        )?;
    }
    if stacks.contains(&StackKind::FlutterDart) {
        run_ext_typecheck(
            repo_root,
            config,
            impacted_files,
            ".dart",
            "dart analyze",
            &["dart", "analyze"],
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
    label: &str,
    base_cmd: &[&str],
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
    let mut args: Vec<&str> = base_cmd.to_vec();
    args.extend(matching.iter().map(String::as_str));
    results.push(run_and_filter(repo_root, config, label, &args, impacted_files)?);
    Ok(())
}

fn run_and_filter(
    repo_root: &Path,
    config: &AnvilConfig,
    label: &str,
    cmd_parts: &[&str],
    impacted_files: &[String],
) -> Result<CompressedOutput> {
    let (program, args) = cmd_parts.split_first().context("empty command")?;
    let output = match Command::new(program).args(args).current_dir(repo_root).output() {
        Ok(out) => out,
        Err(err) => {
            return compress_output(
                Some(repo_root),
                label,
                1,
                &format!("failed to execute `{label}`: {err}"),
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
    let filtered = if impacted_files.is_empty() {
        combined
    } else {
        let kept: Vec<&str> = combined
            .lines()
            .filter(|line| {
                impacted_files
                    .iter()
                    .any(|f| line.contains(f) || line.contains(&f.replace('/', "\\")))
            })
            .collect();
        if kept.is_empty() && code == 0 {
            String::new()
        } else if kept.is_empty() {
            combined
        } else {
            kept.join("\n")
        }
    };

    compress_output(
        Some(repo_root),
        label,
        code,
        &filtered,
        config.tier_budgets.compressor_max_lines,
        config.tier_budgets.compressor_max_tokens,
    )
}
