//! SonarQube CLI (`sonar` / `sonar-scanner`) token-bounded log compressor.

use crate::compressor::{fail_output, pass_output};
use crate::traits::{CompressedOutput, OutputParser};

pub struct SonarParser;

impl OutputParser for SonarParser {
    fn name(&self) -> &'static str {
        "sonar"
    }

    fn matches(&self, command: &str) -> bool {
        let lower = command.to_ascii_lowercase();
        lower.contains("sonar-scanner") || lower.split_whitespace().any(|t| t == "sonar")
    }

    fn parse(&self, _command: &str, exit_code: i32, raw_output: &str) -> CompressedOutput {
        let errors = collect_sonar_lines(raw_output);
        if exit_code == 0 && errors.is_empty() {
            return pass_output(
                self.name(),
                exit_code,
                "PASS (sonar): analysis completed cleanly".to_owned(),
            );
        }
        let lines = if errors.is_empty() {
            raw_output.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect()
        } else {
            errors
        };
        let total = lines.len();
        fail_output(
            self.name(),
            if exit_code == 0 { 1 } else { exit_code },
            format!("FAIL (sonar): {total} issue/error line(s)"),
            lines,
            total,
        )
    }
}

fn collect_sonar_lines(raw_output: &str) -> Vec<String> {
    raw_output
        .lines()
        .map(str::trim)
        .filter(|trimmed| {
            !trimmed.is_empty()
                && (trimmed.starts_with("ERROR:")
                    || trimmed.contains("QUALITY GATE STATUS: FAILED")
                    || trimmed.contains("EXECUTION FAILURE")
                    || trimmed.contains("Failed to execute"))
        })
        .map(str::to_owned)
        .collect()
}
