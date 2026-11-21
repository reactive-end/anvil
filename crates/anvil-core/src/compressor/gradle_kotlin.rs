//! Gradle, Kotlin compiler, ktlint, detekt, and Roborazzi log parser.

use std::sync::OnceLock;

use regex::Regex;

use crate::compressor::{fail_output, pass_output};
use crate::traits::{CompressedOutput, OutputParser};

pub struct GradleKotlinParser;

impl OutputParser for GradleKotlinParser {
    fn name(&self) -> &'static str {
        "gradle/kotlin"
    }

    fn matches(&self, command: &str) -> bool {
        let lower = command.to_ascii_lowercase();
        lower.contains("gradle")
            || lower.contains("kotlinc")
            || lower.contains("ktlint")
            || lower.contains("detekt")
    }

    fn parse(&self, _command: &str, exit_code: i32, raw_output: &str) -> CompressedOutput {
        if exit_code == 0 {
            return pass_output(
                self.name(),
                exit_code,
                "PASS (gradle/kotlin): build/checks succeeded".to_owned(),
            );
        }

        static KOTLIN_ERR_RE: OnceLock<Regex> = OnceLock::new();
        let re = KOTLIN_ERR_RE.get_or_init(|| {
            Regex::new(r"^e:\s+(?:file://)?(.+?:\d+:\d+):?\s*(.+)$")
                .expect("valid kotlin error regex")
        });

        let mut lines = Vec::new();
        for raw_line in raw_output.lines() {
            let trimmed = raw_line.trim();
            if let Some(caps) = re.captures(trimmed) {
                let loc = caps.get(1).map_or("", |m| m.as_str());
                let msg = caps.get(2).map_or("", |m| m.as_str());
                lines.push(format!("{loc} {msg}"));
            } else if trimmed.starts_with("> Task ") && trimmed.contains("FAILED")
                || trimmed.starts_with("FAILURE:")
                || trimmed.contains("AssertionError")
                || trimmed.contains("Roborazzi")
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
            format!("FAIL (gradle/kotlin): {total} error(s)"),
            lines,
            total,
        )
    }
}
