//! Cargo and rustc compiler/test/clippy log parser.

use std::sync::OnceLock;

use regex::Regex;

use crate::compressor::{fail_output, pass_output, strip_ansi};
use crate::traits::{CompressedOutput, OutputParser};

pub struct CargoParser;

impl OutputParser for CargoParser {
    fn name(&self) -> &'static str {
        "cargo"
    }

    fn matches(&self, command: &str) -> bool {
        let mut has_cargo = false;
        let mut has_subcmd = false;
        for tool in command_tool_tokens(command) {
            if tool == "rustc" {
                return true;
            }
            if tool == "cargo" {
                has_cargo = true;
            } else if has_cargo
                && matches!(
                    tool.as_str(),
                    "check" | "test" | "clippy" | "build" | "run" | "bench" | "doc" | "rustc" | "c" | "t"
                )
            {
                has_subcmd = true;
            }
        }
        has_cargo && has_subcmd
    }

    fn parse(&self, _command: &str, exit_code: i32, raw_output: &str) -> CompressedOutput {
        let cleaned = strip_ansi(raw_output);
        if exit_code == 0 {
            let summary_hint = cleaned
                .lines()
                .map(str::trim)
                .find(|l| l.starts_with("test result: ok."))
                .unwrap_or("checks passed");
            return pass_output(self.name(), exit_code, format!("PASS (cargo): {summary_hint}"));
        }

        let mut lines = extract_cargo_diagnostics(&cleaned);
        if lines.is_empty() {
            lines = cleaned
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
        }
        let total = lines
            .iter()
            .filter(|l| l.contains("error") || l.contains("FAILED") || l.contains("panicked"))
            .count()
            .max(1);
        fail_output(
            self.name(),
            exit_code,
            format!("FAIL (cargo): {total} issue(s)"),
            lines,
            total,
        )
    }
}

fn extract_cargo_diagnostics(cleaned: &str) -> Vec<String> {
    static DIAG_RE: OnceLock<Regex> = OnceLock::new();
    static SHORT_RE: OnceLock<Regex> = OnceLock::new();
    static LOC_RE: OnceLock<Regex> = OnceLock::new();
    let diag_re = DIAG_RE.get_or_init(|| {
        Regex::new(r"^(error(?:\[[A-Z0-9]+\])?|warning(?:\[[a-zA-Z0-9_:-]+\])?):\s*(.+)$")
            .expect("valid cargo diag regex")
    });
    let short_re = SHORT_RE.get_or_init(|| {
        Regex::new(r"^(.+?:\d+:\d+):\s*(error(?:\[[A-Z0-9]+\])?|warning(?:\[[a-zA-Z0-9_:-]+\])?):\s*(.+)$")
            .expect("valid cargo short regex")
    });
    let loc_re = LOC_RE.get_or_init(|| {
        Regex::new(r"^\s*-->\s*(.+:\d+:\d+)\s*$").expect("valid cargo loc regex")
    });

    let mut lines = Vec::new();
    let mut pending: Option<(String, String)> = None;

    for raw_line in cleaned.lines() {
        let trimmed = raw_line.trim();
        if let Some(caps) = short_re.captures(trimmed) {
            flush_pending(&mut pending, &mut lines);
            let loc = caps.get(1).map_or("", |m| m.as_str());
            let kind = caps.get(2).map_or("error", |m| m.as_str());
            let msg = caps.get(3).map_or("", |m| m.as_str());
            lines.push(format!("{loc} {kind}: {msg}"));
        } else if let Some(caps) = diag_re.captures(trimmed) {
            flush_pending(&mut pending, &mut lines);
            let kind = caps.get(1).map_or("error", |m| m.as_str());
            let msg = caps.get(2).map_or("", |m| m.as_str());
            if !is_cargo_summary_noise(msg) {
                pending = Some((kind.to_owned(), msg.to_owned()));
            }
        } else if let Some(caps) = loc_re.captures(raw_line) {
            let loc = caps.get(1).map_or("", |m| m.as_str());
            if let Some((kind, msg)) = pending.take() {
                lines.push(format!("{loc} {kind}: {msg}"));
            } else {
                lines.push(format!("--> {loc}"));
            }
        } else if is_test_failure_line(trimmed) {
            flush_pending(&mut pending, &mut lines);
            lines.push(trimmed.to_owned());
        }
    }
    flush_pending(&mut pending, &mut lines);
    lines
}

fn flush_pending(pending: &mut Option<(String, String)>, lines: &mut Vec<String>) {
    if let Some((kind, msg)) = pending.take() {
        lines.push(format!("{kind}: {msg}"));
    }
}

fn is_cargo_summary_noise(msg: &str) -> bool {
    msg.starts_with("could not compile ")
        || msg.starts_with("aborting due to ")
        || (msg.ends_with("warning emitted") || msg.ends_with("warnings emitted"))
}

fn is_test_failure_line(trimmed: &str) -> bool {
    (trimmed.starts_with("test ") && trimmed.ends_with("... FAILED"))
        || trimmed.starts_with("test result: FAILED.")
        || (trimmed.starts_with("thread '") && trimmed.contains("panicked at "))
}

fn command_tool_tokens(command: &str) -> Vec<String> {
    command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')'))
        .filter_map(|raw_tok| {
            let tok = raw_tok.trim_matches(|c| c == '"' || c == '\'');
            if tok.is_empty() {
                return None;
            }
            let base = tok.rsplit(['/', '\\']).next().unwrap_or(tok);
            let lower = base.to_ascii_lowercase();
            let no_ext = lower
                .strip_suffix(".exe")
                .or_else(|| lower.strip_suffix(".cmd"))
                .or_else(|| lower.strip_suffix(".bat"))
                .unwrap_or(&lower);
            Some(no_ext.to_owned())
        })
        .collect()
}
