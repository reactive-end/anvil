//! Godot 4 headless Tier 1 script check runner (`--headless --check-only --script`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::Result;
use anvil_core::compressor::compress_output;
use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::resolve_godot_bin;
use anvil_core::proc_util::run_command_with_timeout;
use anvil_core::traits::CompressedOutput;

use crate::model::CodeGraph;

use super::tests_runner::skip_output;

/// Runs `godot --headless --path <proj> --check-only --script res://<rel>` on impacted `.gd` files.
pub(super) fn run_godot_check(
    log_root: &Path,
    work_dir: &Path,
    config: &AnvilConfig,
    impacted_files: &[String],
) -> Result<CompressedOutput> {
    let Some(godot_bin) = resolve_godot_bin() else {
        return Ok(skip_output("godot", "binary not found"));
    };
    let scripts = collect_target_gd_scripts(work_dir, impacted_files);
    if scripts.is_empty() {
        return Ok(CompressedOutput {
            tool: "godot".to_owned(),
            passed: true,
            exit_code: 0,
            summary: "PASS (godot): 0 GDScript files to check".to_owned(),
            lines: Vec::new(),
            total_errors: 0,
            truncated: false,
            raw_log_path: None,
        });
    }

    let timeout = Duration::from_secs(config.tier_budgets.tier1_secs.max(5));
    let mut combined = String::new();
    let mut exit_code = 0i32;

    for rel in &scripts {
        let (proj_dir, res_rel) = resolve_godot_project_for_script(work_dir, rel);
        let res_arg = format!("res://{res_rel}");
        let mut cmd = Command::new(&godot_bin);
        cmd.arg("--headless")
            .arg("--path")
            .arg(&proj_dir)
            .args(["--check-only", "--script", &res_arg]);
        run_single_godot_check(&mut cmd, &res_arg, timeout, &mut exit_code, &mut combined);
    }

    compress_output(
        Some(log_root),
        "godot --headless --check-only",
        exit_code,
        &combined,
        config.tier_budgets.compressor_max_lines,
        config.tier_budgets.compressor_max_tokens,
    )
}

fn run_single_godot_check(
    cmd: &mut Command,
    res_arg: &str,
    timeout: Duration,
    exit_code: &mut i32,
    combined: &mut String,
) {
    match run_command_with_timeout(cmd, timeout) {
        Ok(out) => {
            if out.status_code != 0 && *exit_code == 0 {
                *exit_code = out.status_code;
            }
            combined.push_str(&String::from_utf8_lossy(&out.stdout));
            combined.push_str(&String::from_utf8_lossy(&out.stderr));
            if !combined.ends_with('\n') {
                combined.push('\n');
            }
        }
        Err(err) => {
            *exit_code = 1;
            combined.push_str(&format!("ERROR: failed to check {res_arg}: {err}\n"));
        }
    }
}

fn collect_target_gd_scripts(work_dir: &Path, impacted_files: &[String]) -> Vec<String> {
    if !impacted_files.is_empty() {
        return impacted_files
            .iter()
            .map(|f| CodeGraph::normalize_rel_path(f))
            .filter(|f| f.ends_with(".gd"))
            .collect();
    }
    let mut found = Vec::new();
    walk_gd_files(work_dir, "", &mut found, 50);
    found
}

fn walk_gd_files(dir: &Path, rel_dir: &str, out: &mut Vec<String>, limit: usize) {
    if out.len() >= limit {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= limit {
            break;
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let rel = if rel_dir.is_empty() {
            name.to_owned()
        } else {
            format!("{rel_dir}/{name}")
        };
        if path.is_dir() {
            if !matches!(name, ".git" | ".godot" | ".harness" | "addons") {
                walk_gd_files(&path, &rel, out, limit);
            }
        } else if name.ends_with(".gd") {
            out.push(rel);
        }
    }
}

pub(super) fn resolve_godot_project_for_script(
    work_dir: &Path,
    rel_script: &str,
) -> (PathBuf, String) {
    let abs = work_dir.join(rel_script);
    let mut cur = abs.parent();
    while let Some(dir) = cur {
        if dir.join("project.godot").is_file() {
            let sub_rel = abs
                .strip_prefix(dir)
                .map(|p| CodeGraph::normalize_rel_path(&p.to_string_lossy()))
                .unwrap_or_else(|_| rel_script.to_owned());
            return (dir.to_path_buf(), sub_rel);
        }
        if dir == work_dir {
            break;
        }
        cur = dir.parent().filter(|p| p.starts_with(work_dir));
    }
    (work_dir.to_path_buf(), rel_script.to_owned())
}
