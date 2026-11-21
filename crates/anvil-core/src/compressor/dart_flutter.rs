//! Dart and Flutter (`dart analyze`, `flutter analyze`, `flutter test`) log parser.

use std::sync::OnceLock;

use regex::Regex;

use crate::compressor::{fail_output, pass_output};
use crate::traits::{CompressedOutput, OutputParser};

pub struct DartFlutterParser;

impl OutputParser for DartFlutterParser {
    fn name(&self) -> &'static str {
        "dart/flutter"
    }

    fn matches(&self, command: &str) -> bool {
        let lower = command.to_ascii_lowercase();
        lower.contains("dart ") || lower == "dart" || lower.contains("flutter")
    }

    fn parse(&self, _command: &str, exit_code: i32, raw_output: &str) -> CompressedOutput {
        if exit_code == 0 {
            return pass_output(
                self.name(),
                exit_code,
                "PASS (dart/flutter): 0 issues".to_owned(),
            );
        }

        static DART_RE: OnceLock<Regex> = OnceLock::new();
        let re = DART_RE.get_or_init(|| {
            Regex::new(r"^(error|warning|info)\s*-\s*(.+?)\s*-\s*(.+?:\d+:\d+)\s*-\s*(\S+)$")
                .expect("valid dart analyze regex")
        });

        let mut lines = Vec::new();
        for raw_line in raw_output.lines() {
            let trimmed = raw_line.trim();
            let normalized = trimmed.replace('•', "-");
            if let Some(caps) = re.captures(&normalized) {
                let sev = caps.get(1).map_or("error", |m| m.as_str());
                let msg = caps.get(2).map_or("", |m| m.as_str()).trim();
                let loc = caps.get(3).map_or("", |m| m.as_str()).trim();
                let code = caps.get(4).map_or("", |m| m.as_str()).trim();
                lines.push(format!("{loc} {sev} [{code}] {msg}"));
            } else if trimmed.contains("[E]") || trimmed.contains("Some tests failed") {
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
            format!("FAIL (dart/flutter): {total} issue(s)"),
            lines,
            total,
        )
    }
}
