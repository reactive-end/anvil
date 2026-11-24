//! `anvil spec`: deterministic spec-driven verification contracts stored under `.harness/specs/`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use anvil_core::proc_util::run_command_with_timeout;

use crate::cli::subcmds::SpecSubcmd;
use crate::commands::ExitCodeError;
use crate::ui;

const SPECS_REL: &str = ".harness/specs";
const CMD_TIMEOUT_SECS: u64 = 30;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    None,
    Files,
    MustContain,
    Forbid,
    Commands,
}

/// Executes `anvil spec <init|list|check>`.
pub fn execute(root: &Path, sub: SpecSubcmd) -> Result<()> {
    match sub {
        SpecSubcmd::Init { name } => init_spec(root, &name),
        SpecSubcmd::List => list_specs(root),
        SpecSubcmd::Check { name } => check_specs(root, name.as_deref()),
    }
}

fn init_spec(root: &Path, name: &str) -> Result<()> {
    let slug = sanitize_slug(name)?;
    let path = root.join(SPECS_REL).join(format!("{slug}.md"));
    if path.exists() {
        bail!("spec already exists: {}", path.display());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let template = format!(
        "# Spec: {name}\n\n## Files\n- src/main.rs\n\n## Must Contain\n<!-- - path/to/file :: required_symbol -->\n\n## Forbid\n<!-- - path/to/file :: forbidden_token -->\n\n## Commands\n<!-- - cargo test -->\n"
    );
    fs::write(&path, template)?;
    println!("{} created {}", ui::green("✓ spec:"), ui::cyan(&path.display().to_string()));
    Ok(())
}

fn list_specs(root: &Path) -> Result<()> {
    let specs = discover_spec_files(root, None)?;
    if specs.is_empty() {
        println!("no specs in {SPECS_REL}/");
        return Ok(());
    }
    for path in specs {
        println!(
            "  - {}",
            ui::cyan(&path.file_stem().unwrap_or_default().to_string_lossy())
        );
    }
    Ok(())
}

fn check_specs(root: &Path, name: Option<&str>) -> Result<()> {
    let specs = discover_spec_files(root, name)?;
    if specs.is_empty() {
        println!("no specs to verify");
        return Ok(());
    }
    let mut failures = Vec::new();
    for spec in &specs {
        verify_single_spec(root, spec, &mut failures)?;
    }
    if failures.is_empty() {
        println!(
            "{} verified {} spec(s)",
            ui::green("✓ spec check: PASS"),
            specs.len()
        );
        return Ok(());
    }
    for f in &failures {
        println!("  {} {f}", ui::error_badge());
    }
    Err(ExitCodeError::with_message(1, format!("spec check failed ({} failure(s))", failures.len())).into())
}

fn verify_single_spec(root: &Path, spec_path: &Path, failures: &mut Vec<String>) -> Result<()> {
    let raw = fs::read_to_string(spec_path)
        .with_context(|| format!("failed to read {}", spec_path.display()))?;
    let spec_name = spec_path.file_name().unwrap_or_default().to_string_lossy();
    let mut section = Section::None;

    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(next) = parse_section_header(trimmed) {
            section = next;
            continue;
        }
        let Some(item) = trimmed.strip_prefix("- ").map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        evaluate_item(root, &spec_name, section, item, failures);
    }
    Ok(())
}

fn parse_section_header(trimmed: &str) -> Option<Section> {
    if !trimmed.starts_with("## ") {
        return None;
    }
    let heading = trimmed.trim_start_matches('#').trim().to_ascii_lowercase();
    Some(match heading.as_str() {
        "files" => Section::Files,
        "must contain" => Section::MustContain,
        "forbid" => Section::Forbid,
        "commands" => Section::Commands,
        _ => Section::None,
    })
}

fn evaluate_item(
    root: &Path,
    spec_name: &str,
    section: Section,
    item: &str,
    failures: &mut Vec<String>,
) {
    match section {
        Section::None => {}
        Section::Files => {
            if !root.join(item).exists() {
                failures.push(format!("[{spec_name}] missing required file `{item}`"));
            }
        }
        Section::MustContain => {
            if let Some((path, pat)) = item.split_once("::") {
                let rel = path.trim();
                let needle = pat.trim();
                let ok = fs::read_to_string(root.join(rel))
                    .map(|c| c.contains(needle))
                    .unwrap_or(false);
                if !ok {
                    failures.push(format!("[{spec_name}] `{rel}` must contain `{needle}`"));
                }
            }
        }
        Section::Forbid => {
            if let Some((path, pat)) = item.split_once("::") {
                let rel = path.trim();
                let needle = pat.trim();
                let present = fs::read_to_string(root.join(rel))
                    .map(|c| c.contains(needle))
                    .unwrap_or(false);
                if present {
                    failures.push(format!("[{spec_name}] `{rel}` contains forbidden `{needle}`"));
                }
            }
        }
        Section::Commands => {
            let mut parts = item.split_whitespace();
            let Some(bin) = parts.next() else {
                return;
            };
            let args: Vec<&str> = parts.collect();
            let mut cmd = Command::new(bin);
            cmd.args(args).current_dir(root);
            match run_command_with_timeout(&mut cmd, Duration::from_secs(CMD_TIMEOUT_SECS)) {
                Ok(out) if !out.timed_out && out.status_code == 0 => {}
                Ok(out) => failures.push(format!(
                    "[{spec_name}] command `{item}` failed (status={:?}, timed_out={})",
                    out.status_code, out.timed_out
                )),
                Err(err) => failures.push(format!("[{spec_name}] command `{item}` error: {err}")),
            }
        }
    }
}

fn discover_spec_files(root: &Path, filter: Option<&str>) -> Result<Vec<PathBuf>> {
    let dir = root.join(SPECS_REL);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    if let Some(name) = filter {
        let slug = sanitize_slug(name)?;
        let candidate = dir.join(format!("{slug}.md"));
        if !candidate.is_file() {
            bail!("spec not found: {}", candidate.display());
        }
        return Ok(vec![candidate]);
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn sanitize_slug(raw: &str) -> Result<String> {
    let slug: String = raw
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        bail!("spec name cannot be empty");
    }
    Ok(trimmed.to_ascii_lowercase())
}
