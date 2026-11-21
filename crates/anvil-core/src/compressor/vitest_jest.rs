//! Vitest and Jest test runner log parser.

use crate::compressor::{fail_output, pass_output};
use crate::traits::{CompressedOutput, OutputParser};

pub struct VitestJestParser;

impl OutputParser for VitestJestParser {
    fn name(&self) -> &'static str {
        "vitest/jest"
    }

    fn matches(&self, command: &str) -> bool {
        let lower = command.to_ascii_lowercase();
        lower.contains("vitest") || lower.contains("jest")
    }

    fn parse(&self, _command: &str, exit_code: i32, raw_output: &str) -> CompressedOutput {
        if exit_code == 0 {
            let summary_hint = raw_output
                .lines()
                .find(|l| l.contains("Tests") || l.contains("passed"))
                .map(str::trim)
                .unwrap_or("all tests passed");
            return pass_output(
                self.name(),
                exit_code,
                format!("PASS (vitest/jest): {summary_hint}"),
            );
        }

        let mut lines = Vec::new();
        for line in raw_output.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("FAIL ")
                || trimmed.starts_with("● ")
                || trimmed.starts_with("AssertionError")
                || trimmed.starts_with("Expected:")
                || trimmed.starts_with("Received:")
                || trimmed.starts_with("at ")
                || trimmed.contains("❯ ")
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

        let total = lines
            .iter()
            .filter(|l| l.starts_with("FAIL ") || l.starts_with("● "))
            .count()
            .max(1);
        fail_output(
            self.name(),
            exit_code,
            format!("FAIL (vitest/jest): {total} failure(s)"),
            lines,
            total,
        )
    }
}
