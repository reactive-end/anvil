//! Targeted Tier 1 test runner executing only impacted test suites via `anvil_core::compressor`.

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::Result;
use anvil_core::compressor::compress_output;
use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::resolve_local_or_path_bin;
use anvil_core::traits::CompressedOutput;

use super::tests_native::{run_kotlin_tests, run_rust_tests};

/// Runs targeted test runners for `related_tests`, skipping gracefully when a runner is absent.
pub fn run_impacted_tests(
    repo_root: &Path,
    config: &AnvilConfig,
    related_tests: &[String],
) -> Result<Vec<CompressedOutput>> {
    if related_tests.is_empty() {
        return Ok(Vec::new());
    }

    let buckets = partition_tests(related_tests);
    let mut results = Vec::new();

    if !buckets.js_ts.is_empty() {
        results.push(run_js_tests(repo_root, config, &buckets.js_ts)?);
    }
    if !buckets.python.is_empty() {
        results.push(run_simple_file_runner(repo_root, config, "pytest", &[], &buckets.python)?);
    }
    if !buckets.dart.is_empty() {
        results.push(run_dart_flutter_tests(repo_root, config, &buckets.dart)?);
    }
    if !buckets.kotlin.is_empty() {
        results.push(run_kotlin_tests(repo_root, config, &buckets.kotlin)?);
    }
    if !buckets.rust.is_empty() {
        results.extend(run_rust_tests(repo_root, config, &buckets.rust)?);
    }
    Ok(results)
}

#[derive(Default)]
struct TestBuckets<'a> {
    js_ts: Vec<&'a str>,
    python: Vec<&'a str>,
    dart: Vec<&'a str>,
    kotlin: Vec<&'a str>,
    rust: Vec<&'a str>,
}

fn partition_tests<'a>(related_tests: &'a [String]) -> TestBuckets<'a> {
    let mut buckets = TestBuckets::default();
    for raw in related_tests {
        let lower = raw.to_ascii_lowercase();
        if lower.ends_with(".py") {
            buckets.python.push(raw.as_str());
        } else if lower.ends_with(".dart") {
            buckets.dart.push(raw.as_str());
        } else if lower.ends_with(".kt") {
            buckets.kotlin.push(raw.as_str());
        } else if lower.ends_with(".rs") {
            buckets.rust.push(raw.as_str());
        } else if lower.ends_with(".ts")
            || lower.ends_with(".tsx")
            || lower.ends_with(".js")
            || lower.ends_with(".jsx")
            || lower.ends_with(".mjs")
            || lower.ends_with(".cjs")
        {
            buckets.js_ts.push(raw.as_str());
        }
    }
    buckets
}

fn run_js_tests(
    repo_root: &Path,
    config: &AnvilConfig,
    files: &[&str],
) -> Result<CompressedOutput> {
    if let Some(vitest) = resolve_local_or_path_bin(repo_root, "vitest") {
        let mut args = vec!["related", "--run"];
        args.extend_from_slice(files);
        return exec_compressed(repo_root, repo_root, config, &vitest, "vitest related", &args);
    }
    if let Some(jest) = resolve_local_or_path_bin(repo_root, "jest") {
        let mut args = vec!["--findRelatedTests"];
        args.extend_from_slice(files);
        return exec_compressed(
            repo_root,
            repo_root,
            config,
            &jest,
            "jest --findRelatedTests",
            &args,
        );
    }
    Ok(skip_output("vitest/jest", "test runner binary not found"))
}

fn run_dart_flutter_tests(
    repo_root: &Path,
    config: &AnvilConfig,
    files: &[&str],
) -> Result<CompressedOutput> {
    let uses_flutter = fs::read_to_string(repo_root.join("pubspec.yaml"))
        .is_ok_and(|s| s.contains("flutter:"));
    if uses_flutter
        && let Some(flutter) = resolve_local_or_path_bin(repo_root, "flutter")
    {
        let mut args = vec!["test"];
        args.extend_from_slice(files);
        return exec_compressed(repo_root, repo_root, config, &flutter, "flutter test", &args);
    }
    if let Some(dart) = resolve_local_or_path_bin(repo_root, "dart") {
        let mut args = vec!["test"];
        args.extend_from_slice(files);
        return exec_compressed(repo_root, repo_root, config, &dart, "dart test", &args);
    }
    Ok(skip_output("dart/flutter", "test runner binary not found"))
}

fn run_simple_file_runner(
    repo_root: &Path,
    config: &AnvilConfig,
    bin: &str,
    base_args: &[&str],
    files: &[&str],
) -> Result<CompressedOutput> {
    let Some(bin_path) = resolve_local_or_path_bin(repo_root, bin) else {
        return Ok(skip_output(bin, "binary not found"));
    };
    let mut args = base_args.to_vec();
    args.extend_from_slice(files);
    exec_compressed(repo_root, repo_root, config, &bin_path, bin, &args)
}

pub(super) fn exec_compressed(
    log_root: &Path,
    work_dir: &Path,
    config: &AnvilConfig,
    bin_path: &Path,
    label: &str,
    args: &[&str],
) -> Result<CompressedOutput> {
    let output = match Command::new(bin_path).args(args).current_dir(work_dir).output() {
        Ok(out) => out,
        Err(err) => {
            return compress_output(
                Some(log_root),
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
    compress_output(
        Some(log_root),
        label,
        code,
        &combined,
        config.tier_budgets.compressor_max_lines,
        config.tier_budgets.compressor_max_tokens,
    )
}

pub(super) fn skip_output(tool: &str, reason: &str) -> CompressedOutput {
    CompressedOutput {
        tool: tool.to_owned(),
        passed: true,
        exit_code: 0,
        summary: format!("SKIP ({tool}): {reason}"),
        lines: Vec::new(),
        total_errors: 0,
        truncated: false,
        raw_log_path: None,
    }
}
