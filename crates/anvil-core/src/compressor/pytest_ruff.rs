//! Pytest and Ruff log parser.

use std::sync::OnceLock;

use regex::Regex;

use crate::compressor::{fail_output, pass_output};
use crate::traits::{CompressedOutput, OutputParser};

pub struct PytestRuffParser;

impl OutputParser for PytestRuffParser {
    fn name(&self) -> &'static str {
        "pytest/ruff"
    }

    fn matches(&self, command: &str) -> bool {
        let lower = command.to_ascii_lowercase();
        lower.contains("pytest") || lower.contains("ruff")
    }

    fn parse(&self, _command: &str, exit_code: i32, raw_output: &str) -> CompressedOutput {
        if exit_code == 0 {
            return pass_output(
                self.name(),
                exit_code,
                "PASS (pytest/ruff): checks passed".to_owned(),
            );
        }

        static RUFF_RE: OnceLock<Regex> = OnceLock::new();
        let ruff_re = RUFF_RE.get_or_init(|| {
            Regex::new(r"^(.+?:\d+:\d+):\s+([A-Z]+\d+)\s+(.+)$").expect("valid ruff regex")
        });

        let mut lines = Vec::new();
        for raw_line in raw_output.lines() {
            let trimmed = raw_line.trim();
            if let Some(caps) = ruff_re.captures(trimmed) {
                let loc = caps.get(1).map_or("", |m| m.as_str());
                let code = caps.get(2).map_or("", |m| m.as_str());
                let msg = caps.get(3).map_or("", |m| m.as_str());
                lines.push(format!("{loc} [{code}] {msg}"));
            } else if trimmed.starts_with("FAILED ")
                || trimmed.starts_with("ERROR ")
                || trimmed.starts_with("E   ")
                || trimmed.starts_with("AssertionError")
            {
                lines.push(trimmed.to_owned());
            }
        }
        if lines.is_empty() {
            lines = raw_output
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
        }
        let total = lines.len();
        fail_output(
            self.name(),
            exit_code,
            format!("FAIL (pytest/ruff): {total} issue(s)"),
            lines,
            total,
        )
    }
}
