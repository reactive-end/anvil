//! Filtered native compiler/typechecker invocation compressed via `anvil_core::compressor`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use anvil_core::compressor::compress_output;
use anvil_core::config::{AnvilConfig, StackKind};
use anvil_core::fs_util::resolve_local_or_path_bin;
use anvil_core::stack_detector::detect_workspace;
use anvil_core::traits::CompressedOutput;

use crate::model::CodeGraph;

pub use super::typecheck_filter::filter_typecheck_output;

struct TypecheckCtx<'a> {
    log_root: &'a Path,
    work_dir: &'a Path,
    config: &'a AnvilConfig,
    impacted_files: &'a [String],
}

/// Runs native compiler/typechecker filtered to `impacted_files` across repo or `config.subrepos`.
pub fn run_impacted_typecheck(
    repo_root: &Path,
    config: &AnvilConfig,
    impacted_files: &[String],
) -> Result<Vec<CompressedOutput>> {
    let mut results = Vec::new();
    if config.subrepos.is_empty() {
        let stacks = resolve_stacks(repo_root, config);
        let ctx = TypecheckCtx { log_root: repo_root, work_dir: repo_root, config, impacted_files };
        run_stacks_in_dir(&ctx, &stacks, &mut results)?;
        return Ok(results);
    }

    for sub in &config.subrepos {
        let sub_norm = CodeGraph::normalize_rel_path(&sub.path);
        let sub_prefix = sub_norm.trim_end_matches('/');
        let sub_dir = if sub_prefix.is_empty() || sub_prefix == "." {
            repo_root.to_path_buf()
        } else {
            repo_root.join(sub_prefix)
        };
        if !sub_dir.is_dir() {
            continue;
        }
        let sub_impacted = if impacted_files.is_empty() {
            Vec::new()
        } else {
            let scoped = scope_files_to_subrepo(impacted_files, sub_prefix);
            if scoped.is_empty() {
                continue;
            }
            scoped
        };
        let stacks = if sub.stacks.is_empty() {
            detect_subrepo_stacks(&sub_dir, config)
        } else {
            sub.stacks.clone()
        };
        let ctx = TypecheckCtx {
            log_root: repo_root,
            work_dir: &sub_dir,
            config,
            impacted_files: &sub_impacted,
        };
        run_stacks_in_dir(&ctx, &stacks, &mut results)?;
    }
    Ok(results)
}

fn run_stacks_in_dir(
    ctx: &TypecheckCtx<'_>,
    stacks: &[StackKind],
    results: &mut Vec<CompressedOutput>,
) -> Result<()> {
    if stacks.iter().any(is_ts_stack) {
        let bin = resolve_local_or_path_bin(ctx.work_dir, "tsc");
        results.push(run_resolved_and_filter(ctx, bin, "tsc", "tsc --noEmit", &["--noEmit"])?);
    }
    if stacks.contains(&StackKind::Python) {
        run_ext_typecheck(ctx, ".py", "pyright", &[], results)?;
    }
    if stacks.contains(&StackKind::FlutterDart) {
        run_ext_typecheck(ctx, ".dart", "dart", &["analyze"], results)?;
    }
    if stacks.contains(&StackKind::Rust)
        && (ctx.impacted_files.is_empty() || ctx.impacted_files.iter().any(|f| f.ends_with(".rs")))
    {
        let bin = resolve_local_or_path_bin(ctx.work_dir, "cargo");
        results.push(run_resolved_and_filter(
            ctx,
            bin,
            "cargo",
            "cargo check",
            &["check", "--message-format=short"],
        )?);
    }
    if stacks.contains(&StackKind::KotlinAndroid)
        && (ctx.impacted_files.is_empty()
            || ctx.impacted_files.iter().any(|f| f.ends_with(".kt") || f.ends_with(".kts")))
    {
        let bin = resolve_gradle_bin(ctx.work_dir);
        results.push(run_resolved_and_filter(
            ctx,
            bin,
            "gradlew",
            "gradlew compileDebugKotlin",
            &["compileDebugKotlin"],
        )?);
    }
    Ok(())
}

pub(crate) fn resolve_gradle_bin(work_dir: &Path) -> Option<PathBuf> {
    if cfg!(windows) {
        let bat = work_dir.join("gradlew.bat");
        if bat.is_file() {
            return Some(bat);
        }
    }
    let sh = work_dir.join("gradlew");
    if sh.is_file() {
        return Some(sh);
    }
    resolve_local_or_path_bin(work_dir, "gradle")
}

fn scope_files_to_subrepo(files: &[String], sub_prefix: &str) -> Vec<String> {
    if sub_prefix.is_empty() || sub_prefix == "." {
        return files.to_vec();
    }
    let with_slash = format!("{sub_prefix}/");
    files
        .iter()
        .filter_map(|f| CodeGraph::normalize_rel_path(f).strip_prefix(&with_slash).map(str::to_owned))
        .collect()
}

fn resolve_stacks(repo_root: &Path, config: &AnvilConfig) -> Vec<StackKind> {
    if !config.stacks.is_empty() {
        return config.stacks.clone();
    }
    detect_workspace(repo_root)
        .map(|d| d.repos.into_iter().flat_map(|r| r.stacks).collect())
        .unwrap_or_default()
}

fn detect_subrepo_stacks(sub_dir: &Path, config: &AnvilConfig) -> Vec<StackKind> {
    let detected: Vec<StackKind> = detect_workspace(sub_dir)
        .map(|d| d.repos.into_iter().flat_map(|r| r.stacks).collect())
        .unwrap_or_default();
    if detected.is_empty() { config.stacks.clone() } else { detected }
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
    ctx: &TypecheckCtx<'_>,
    ext: &str,
    bin: &str,
    base_args: &[&str],
    results: &mut Vec<CompressedOutput>,
) -> Result<()> {
    let matching: Vec<String> =
        ctx.impacted_files.iter().filter(|f| f.ends_with(ext)).cloned().collect();
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
    let bin_path = resolve_local_or_path_bin(ctx.work_dir, bin);
    results.push(run_resolved_and_filter(ctx, bin_path, bin, &label, &args)?);
    Ok(())
}

fn run_resolved_and_filter(
    ctx: &TypecheckCtx<'_>,
    bin_path: Option<PathBuf>,
    bin_name: &str,
    label: &str,
    args: &[&str],
) -> Result<CompressedOutput> {
    let Some(bin_path) = bin_path else {
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

    let output = match Command::new(&bin_path).args(args).current_dir(ctx.work_dir).output() {
        Ok(out) => out,
        Err(err) => {
            return compress_output(
                Some(ctx.log_root),
                label,
                1,
                &format!("failed to execute `{}`: {err}", bin_path.display()),
                ctx.config.tier_budgets.compressor_max_lines,
                ctx.config.tier_budgets.compressor_max_tokens,
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
        Some(ctx.log_root),
        label,
        code,
        &combined,
        ctx.impacted_files,
        ctx.config.tier_budgets.compressor_max_lines,
        ctx.config.tier_budgets.compressor_max_tokens,
    )
}
