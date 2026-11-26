//! Context-bounded output compression (`anvil run`) and raw log archival.

pub mod cargo;
pub mod dart_flutter;
pub mod eslint_biome;
pub mod generic;
pub mod godot;
pub mod gradle_kotlin;
pub mod php;
pub mod pytest_ruff;
pub mod sonar;
pub mod tsc;
pub mod vitest_jest;

use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::Regex;
use ulid::Ulid;

use crate::proc_util::{TIMEOUT_EXIT_CODE, TIMEOUT_MARKER};
use crate::traits::{CompressedOutput, OutputParser};

pub const DEFAULT_MAX_LINES: usize = 50;
pub const DEFAULT_MAX_TOKENS: usize = 1200;

/// Compresses command output using a matching parser and enforces hard line/token caps.
/// On failure, writes the raw full output to `.harness/.state/logs/<id>.log` when `repo_root` is provided.
pub fn compress_output(
    repo_root: Option<&Path>,
    command: &str,
    exit_code: i32,
    raw_output: &str,
    max_lines: usize,
    max_tokens: usize,
) -> Result<CompressedOutput> {
    let cleaned = strip_ansi(raw_output);
    let mut parsed = select_parser(command).parse(command, exit_code, &cleaned);

    if let Some(limit) = timeout_limit(exit_code, &cleaned) {
        mark_timed_out(&mut parsed, limit);
    }

    if parsed.passed {
        parsed.lines.clear();
        parsed.truncated = false;
        return Ok(parsed);
    }

    let (capped, truncated) = cap_lines(&parsed.lines, max_lines, max_tokens);
    parsed.lines = capped;
    parsed.truncated |= truncated;

    if let Some(root) = repo_root {
        let log_path = write_raw_log(root, raw_output)?;
        parsed.raw_log_path = Some(log_path);
    }
    Ok(parsed)
}

/// Extracts the limit from the marker `proc_util` appends when it kills a process.
fn timeout_limit(exit_code: i32, output: &str) -> Option<&str> {
    if exit_code != TIMEOUT_EXIT_CODE {
        return None;
    }
    output.rsplit_once(TIMEOUT_MARKER)?.1.split_whitespace().next()
}

/// Reports an interrupted run as a timeout rather than as whatever the partial output looked like.
fn mark_timed_out(parsed: &mut CompressedOutput, limit: &str) {
    let note = format!(
        "TIMEOUT: killed after {limit} before finishing; results are partial (raise `tier_budgets.tier1_timeout_secs`)"
    );
    parsed.passed = false;
    parsed.summary = format!("{note}. {}", parsed.summary);
    parsed.lines.insert(0, note);
}

/// Strips ANSI CSI color/formatting escape sequences (`\x1b[...m` etc.).
pub fn strip_ansi(input: &str) -> Cow<'_, str> {
    if !input.contains('\x1b') {
        return Cow::Borrowed(input);
    }
    static ANSI_RE: OnceLock<Regex> = OnceLock::new();
    let re =
        ANSI_RE.get_or_init(|| Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").expect("valid ansi regex"));
    re.replace_all(input, "")
}

/// Selects the most specific parser for `command`, falling back to `generic`.
pub fn select_parser(command: &str) -> Box<dyn OutputParser> {
    let parsers: [Box<dyn OutputParser>; 10] = [
        Box::new(cargo::CargoParser),
        Box::new(tsc::TscParser),
        Box::new(eslint_biome::EslintBiomeParser),
        Box::new(vitest_jest::VitestJestParser),
        Box::new(pytest_ruff::PytestRuffParser),
        Box::new(gradle_kotlin::GradleKotlinParser),
        Box::new(dart_flutter::DartFlutterParser),
        Box::new(php::PhpParser),
        Box::new(godot::GodotParser),
        Box::new(sonar::SonarParser),
    ];
    for parser in parsers {
        if parser.matches(command) {
            return parser;
        }
    }
    Box::new(generic::GenericParser)
}

pub(crate) fn pass_output(tool: &str, exit_code: i32, summary: String) -> CompressedOutput {
    CompressedOutput {
        tool: tool.to_owned(),
        passed: true,
        exit_code,
        summary,
        lines: Vec::new(),
        total_errors: 0,
        truncated: false,
        raw_log_path: None,
    }
}

pub(crate) fn fail_output(
    tool: &str,
    exit_code: i32,
    summary: String,
    lines: Vec<String>,
    total_errors: usize,
) -> CompressedOutput {
    CompressedOutput {
        tool: tool.to_owned(),
        passed: false,
        exit_code,
        summary,
        lines,
        total_errors,
        truncated: false,
        raw_log_path: None,
    }
}

/// Enforces hard caps on line count and approximate token budget (~4 chars/token).
pub(crate) fn cap_lines(
    lines: &[String],
    max_lines: usize,
    max_tokens: usize,
) -> (Vec<String>, bool) {
    let char_budget = max_tokens.saturating_mul(4);
    let mut out = Vec::with_capacity(lines.len().min(max_lines));
    let mut used_chars = 0usize;
    let mut truncated = false;

    for line in lines {
        if out.len() >= max_lines {
            truncated = true;
            break;
        }
        let next_chars = used_chars.saturating_add(line.len()).saturating_add(1);
        if next_chars > char_budget {
            truncated = true;
            if out.is_empty() && char_budget > 1 {
                let clipped = truncate_line_utf8(line, char_budget - 1);
                out.push(clipped);
            }
            break;
        }
        used_chars = next_chars;
        out.push(line.clone());
    }
    (out, truncated)
}

fn truncate_line_utf8(line: &str, max_bytes: usize) -> String {
    if line.len() <= max_bytes {
        return line.to_owned();
    }
    if max_bytes <= 3 {
        let mut end = max_bytes.min(line.len());
        while end > 0 && !line.is_char_boundary(end) {
            end -= 1;
        }
        return line[..end].to_owned();
    }
    let mut end = (max_bytes - 3).min(line.len());
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &line[..end])
}

fn write_raw_log(repo_root: &Path, raw_output: &str) -> Result<PathBuf> {
    let logs_dir = repo_root.join(".harness").join(".state").join("logs");
    fs::create_dir_all(&logs_dir)
        .with_context(|| format!("failed to create {}", logs_dir.display()))?;
    let path = logs_dir.join(format!("{}.log", Ulid::new()));
    fs::write(&path, raw_output)
        .with_context(|| format!("failed to write raw log at {}", path.display()))?;
    prune_old_logs(&logs_dir, 50);
    Ok(path)
}

fn prune_old_logs(logs_dir: &Path, max_keep: usize) {
    let Ok(entries) = fs::read_dir(logs_dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("log"))
        .collect();
    if logs.len() <= max_keep {
        return;
    }
    logs.sort();
    let excess = logs.len() - max_keep;
    for old in logs.into_iter().take(excess) {
        let _ = fs::remove_file(old);
    }
}
