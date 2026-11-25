//! `anvil spec`: deterministic spec-driven verification contracts stored under `.harness/specs/`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use anvil_core::fs_util::resolve_local_or_path_bin;
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
        Section::Files => match resolve_contained_spec_path(root, item) {
            Ok(abs) if abs.exists() => {}
            Ok(_) => failures.push(format!("[{spec_name}] missing required file `{item}`")),
            Err(err) => failures.push(format!("[{spec_name}] invalid file path `{item}`: {err}")),
        },
        Section::MustContain => evaluate_must_contain(root, spec_name, item, failures),
        Section::Forbid => evaluate_forbid(root, spec_name, item, failures),
        Section::Commands => evaluate_command(root, spec_name, item, failures),
    }
}

fn evaluate_must_contain(root: &Path, spec_name: &str, item: &str, failures: &mut Vec<String>) {
    let Some((path, pat)) = item.split_once("::") else {
        return;
    };
    let (rel, needle) = (path.trim(), pat.trim());
    let Ok(abs) = resolve_contained_spec_path(root, rel) else {
        failures.push(format!("[{spec_name}] path `{rel}` escapes repository root"));
        return;
    };
    let ok = fs::read_to_string(&abs)
        .map(|c| c.contains(needle))
        .unwrap_or(false);
    if !ok {
        failures.push(format!("[{spec_name}] `{rel}` must contain `{needle}`"));
    }
}

fn evaluate_forbid(root: &Path, spec_name: &str, item: &str, failures: &mut Vec<String>) {
    let Some((path, pat)) = item.split_once("::") else {
        return;
    };
    let (rel, needle) = (path.trim(), pat.trim());
    let Ok(abs) = resolve_contained_spec_path(root, rel) else {
        failures.push(format!("[{spec_name}] path `{rel}` escapes repository root"));
        return;
    };
    let Ok(bytes) = fs::read(&abs) else {
        return;
    };
    match String::from_utf8(bytes) {
        Ok(content) => {
            if content.contains(needle) {
                failures.push(format!("[{spec_name}] `{rel}` contains forbidden `{needle}`"));
            }
        }
        Err(_) => failures.push(format!(
            "[{spec_name}] `{rel}` is non-UTF-8; cannot verify absence of `{needle}`"
        )),
    }
}

fn evaluate_command(root: &Path, spec_name: &str, item: &str, failures: &mut Vec<String>) {
    let tokens = split_spec_command(item);
    let Some(bin) = tokens.first() else {
        return;
    };
    let resolved = resolve_local_or_path_bin(root, bin).unwrap_or_else(|| PathBuf::from(bin));
    let mut cmd = Command::new(&resolved);
    cmd.args(&tokens[1..]).current_dir(root);
    match run_command_with_timeout(&mut cmd, Duration::from_secs(CMD_TIMEOUT_SECS)) {
        Ok(out) if !out.timed_out && out.status_code == 0 => {}
        Ok(out) => failures.push(format!(
            "[{spec_name}] command `{item}` failed (status={:?}, timed_out={})",
            out.status_code, out.timed_out
        )),
        Err(err) => failures.push(format!("[{spec_name}] command `{item}` error: {err}")),
    }
}

fn resolve_contained_spec_path(root: &Path, raw: &str) -> Result<PathBuf> {
    let cand = Path::new(raw);
    if cand.is_absolute() {
        bail!("absolute paths are forbidden in specs");
    }
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let joined = root.join(cand);
    let target_canon = if let Ok(c) = joined.canonicalize() {
        c
    } else {
        let mut cur = joined.as_path();
        let mut tail = Vec::new();
        while !cur.exists() {
            if let Some(name) = cur.file_name() {
                tail.push(name.to_os_string());
            }
            let Some(parent) = cur.parent() else {
                break;
            };
            cur = parent;
        }
        let mut base = cur.canonicalize().unwrap_or_else(|_| cur.to_path_buf());
        for seg in tail.into_iter().rev() {
            base.push(seg);
        }
        base
    };
    if !target_canon.starts_with(&root_canon) {
        bail!("path `{raw}` resolves outside repository root");
    }
    Ok(target_canon)
}

fn split_spec_command(raw: &str) -> Vec<String> {
    let (mut out, mut cur, mut quote) = (Vec::new(), String::new(), None::<char>);
    for ch in raw.chars() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                cur.push(ch);
            }
        } else if matches!(ch, '"' | '\'') {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
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
