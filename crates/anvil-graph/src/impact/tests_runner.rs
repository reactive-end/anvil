//! Targeted Tier 1 test runner executing only impacted test suites via `anvil_core::compressor`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::Result;
use anvil_core::compressor::compress_output;
use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::{resolve_godot_bin, resolve_local_or_path_bin};
use anvil_core::proc_util::run_command_with_timeout;
use anvil_core::traits::CompressedOutput;

use super::tests_native::{run_kotlin_tests, run_php_tests, run_rust_tests};
use super::typecheck_godot::resolve_godot_project_for_script;

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
        results.extend(run_js_tests(repo_root, config, &buckets.js_ts)?);
    }
    if !buckets.python.is_empty() {
        results.extend(run_simple_file_runner(repo_root, config, "pytest", &[], &buckets.python)?);
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
    if !buckets.php.is_empty() {
        results.extend(run_php_tests(repo_root, config, &buckets.php)?);
    }
    if !buckets.godot.is_empty() {
        results.push(run_godot_gut_tests(repo_root, config, &buckets.godot)?);
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
    php: Vec<&'a str>,
    godot: Vec<&'a str>,
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
        } else if lower.ends_with(".php") && !lower.ends_with(".blade.php") {
            buckets.php.push(raw.as_str());
        } else if is_godot_test_path(&lower) {
            buckets.godot.push(raw.as_str());
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
) -> Result<Vec<CompressedOutput>> {
    let mut outputs = Vec::new();
    for (work_dir, rel_files) in group_by_subrepo(repo_root, config, files) {
        let refs: Vec<&str> = rel_files.iter().map(String::as_str).collect();
        if let Some(vitest) = resolve_bin_subrepo_or_root(&work_dir, repo_root, "vitest") {
            let mut args = vec!["related", "--run"];
            args.extend_from_slice(&refs);
            outputs.push(exec_compressed(repo_root, &work_dir, config, &vitest, "vitest related", &args)?);
        } else if let Some(jest) = resolve_bin_subrepo_or_root(&work_dir, repo_root, "jest") {
            let mut args = vec!["--findRelatedTests"];
            args.extend_from_slice(&refs);
            outputs.push(exec_compressed(
                repo_root,
                &work_dir,
                config,
                &jest,
                "jest --findRelatedTests",
                &args,
            )?);
        } else if uses_node_test(&work_dir, repo_root) {
            if let Some(node) = resolve_bin_subrepo_or_root(&work_dir, repo_root, "node") {
                let test_refs: Vec<&str> = refs.iter().copied().filter(|p| is_js_test_path(p)).collect();
                let mut args = vec!["--test"];
                args.extend_from_slice(&test_refs);
                outputs.push(exec_compressed(
                    repo_root,
                    &work_dir,
                    config,
                    &node,
                    "node --test",
                    &args,
                )?);
            } else {
                outputs.push(skip_output("node --test", "node binary not found"));
            }
        } else {
            outputs.push(skip_output("vitest/jest", "test runner binary not found"));
        }
    }
    Ok(outputs)
}

fn uses_node_test(work_dir: &Path, repo_root: &Path) -> bool {
    let sub_pkg = work_dir.join("package.json");
    if sub_pkg.is_file() {
        return package_uses_node_test(&sub_pkg);
    }
    work_dir != repo_root && package_uses_node_test(&repo_root.join("package.json"))
}

fn package_uses_node_test(pkg_path: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(pkg_path) else {
        return false;
    };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    val.get("scripts")
        .and_then(|s| s.as_object())
        .is_some_and(|scripts| {
            scripts
                .iter()
                .filter(|(k, _)| *k == "test" || k.starts_with("test:"))
                .filter_map(|(_, v)| v.as_str())
                .any(cmd_uses_node_test)
        })
}

fn cmd_uses_node_test(cmd: &str) -> bool {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    parts.iter().any(|p| *p == "node" || *p == "node.exe")
        && parts.iter().any(|p| *p == "--test" || p.starts_with("--test="))
}

fn is_js_test_path(raw: &str) -> bool {
    let lower = raw.replace('\\', "/").to_ascii_lowercase();
    let fname = lower.rsplit('/').next().unwrap_or(&lower);
    lower.starts_with("test/")
        || lower.starts_with("tests/")
        || lower.contains("/test/")
        || lower.contains("/tests/")
        || fname.contains(".test.")
        || fname.contains(".spec.")
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
) -> Result<Vec<CompressedOutput>> {
    let mut outputs = Vec::new();
    for (work_dir, rel_files) in group_by_subrepo(repo_root, config, files) {
        let Some(bin_path) = resolve_bin_subrepo_or_root(&work_dir, repo_root, bin) else {
            outputs.push(skip_output(bin, "binary not found"));
            continue;
        };
        let mut args = base_args.to_vec();
        let refs: Vec<&str> = rel_files.iter().map(String::as_str).collect();
        args.extend_from_slice(&refs);
        outputs.push(exec_compressed(repo_root, &work_dir, config, &bin_path, bin, &args)?);
    }
    Ok(outputs)
}

fn resolve_bin_subrepo_or_root(work_dir: &Path, repo_root: &Path, bin: &str) -> Option<PathBuf> {
    resolve_local_or_path_bin(work_dir, bin).or_else(|| {
        (work_dir != repo_root)
            .then(|| resolve_local_or_path_bin(repo_root, bin))
            .flatten()
    })
}

fn group_by_subrepo(
    repo_root: &Path,
    config: &AnvilConfig,
    files: &[&str],
) -> Vec<(PathBuf, Vec<String>)> {
    let mut grouped: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    for &file in files {
        let norm = file.trim_start_matches("./").replace('\\', "/");
        let matched = config
            .subrepos
            .iter()
            .filter(|s| s.path != "." && !s.path.is_empty())
            .filter_map(|s| {
                let sub_norm = s.path.trim_matches(['/', '\\']).replace('\\', "/");
                norm.strip_prefix(&format!("{sub_norm}/"))
                    .map(|rest| (sub_norm.len(), s.path.clone(), rest.to_owned()))
            })
            .max_by_key(|(len, _, _)| *len);
        if let Some((_, sub_path, stripped)) = matched {
            grouped.entry(repo_root.join(sub_path)).or_default().push(stripped);
        } else {
            grouped
                .entry(repo_root.to_path_buf())
                .or_default()
                .push((*file).to_owned());
        }
    }
    grouped.into_iter().collect()
}

pub(super) fn exec_compressed(
    log_root: &Path,
    work_dir: &Path,
    config: &AnvilConfig,
    bin_path: &Path,
    label: &str,
    args: &[&str],
) -> Result<CompressedOutput> {
    let timeout = Duration::from_secs(config.tier_budgets.tier1_secs.max(5));
    let mut cmd = Command::new(bin_path);
    cmd.args(args).current_dir(work_dir);
    let timed = match run_command_with_timeout(&mut cmd, timeout) {
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
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&timed.stdout),
        String::from_utf8_lossy(&timed.stderr)
    );
    compress_output(
        Some(log_root),
        label,
        timed.status_code,
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

fn is_godot_test_path(lower: &str) -> bool {
    if !lower.ends_with(".gd") {
        return false;
    }
    let fname = lower.rsplit('/').next().unwrap_or(lower);
    lower.starts_with("test/")
        || lower.starts_with("tests/")
        || lower.contains("/test/")
        || lower.contains("/tests/")
        || fname.ends_with("_test.gd")
        || fname.starts_with("test_")
}

fn run_godot_gut_tests(
    repo_root: &Path,
    config: &AnvilConfig,
    files: &[&str],
) -> Result<CompressedOutput> {
    let (proj_dir, _) = resolve_godot_project_for_script(repo_root, files[0]);
    if !proj_dir.join("addons/gut/gut_cmdln.gd").is_file() {
        return Ok(skip_output("godot-gut", "addons/gut/gut_cmdln.gd not found"));
    }
    let Some(bin) = resolve_godot_bin() else {
        return Ok(skip_output("godot-gut", "godot binary not found"));
    };
    let res_tests: Vec<String> = files
        .iter()
        .map(|f| {
            let (_, sub_rel) = resolve_godot_project_for_script(repo_root, f);
            format!("res://{sub_rel}")
        })
        .collect();
    let gtest_arg = format!("-gtest={}", res_tests.join(","));
    let proj_str = proj_dir.to_string_lossy().into_owned();
    let args = [
        "--headless",
        "-s",
        "--path",
        &proj_str,
        "addons/gut/gut_cmdln.gd",
        "-gexit",
        &gtest_arg,
    ];
    exec_compressed(repo_root, &proj_dir, config, &bin, "godot gut", &args)
}
