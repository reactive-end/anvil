//! Context-bounded output compression (`anvil run`) and raw log archival.

pub mod dart_flutter;
pub mod eslint_biome;
pub mod generic;
pub mod gradle_kotlin;
pub mod pytest_ruff;
pub mod tsc;
pub mod vitest_jest;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ulid::Ulid;

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
    let mut parsed = select_parser(command).parse(command, exit_code, raw_output);

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

/// Selects the most specific parser for `command`, falling back to `generic`.
pub fn select_parser(command: &str) -> Box<dyn OutputParser> {
    let parsers: [Box<dyn OutputParser>; 6] = [
        Box::new(tsc::TscParser),
        Box::new(eslint_biome::EslintBiomeParser),
        Box::new(vitest_jest::VitestJestParser),
        Box::new(pytest_ruff::PytestRuffParser),
        Box::new(gradle_kotlin::GradleKotlinParser),
        Box::new(dart_flutter::DartFlutterParser),
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
pub(crate) fn cap_lines(lines: &[String], max_lines: usize, max_tokens: usize) -> (Vec<String>, bool) {
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
        if !out.is_empty() && next_chars > char_budget {
            truncated = true;
            break;
        }
        used_chars = next_chars;
        out.push(line.clone());
    }
    (out, truncated)
}

fn write_raw_log(repo_root: &Path, raw_output: &str) -> Result<PathBuf> {
    let logs_dir = repo_root.join(".harness").join(".state").join("logs");
    fs::create_dir_all(&logs_dir)
        .with_context(|| format!("failed to create {}", logs_dir.display()))?;
    let path = logs_dir.join(format!("{}.log", Ulid::new()));
    fs::write(&path, raw_output)
        .with_context(|| format!("failed to write raw log at {}", path.display()))?;
    Ok(path)
}
