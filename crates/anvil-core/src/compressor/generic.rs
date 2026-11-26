//! Generic fallback log parser for commands without a specialized parser.

use crate::compressor::{fail_output, pass_output};
use crate::traits::{CompressedOutput, OutputParser};

pub struct GenericParser;

impl OutputParser for GenericParser {
    fn name(&self) -> &'static str {
        "generic"
    }

    fn matches(&self, _command: &str) -> bool {
        true
    }

    fn parse(&self, command: &str, exit_code: i32, raw_output: &str) -> CompressedOutput {
        let cmd_name = command.split_whitespace().next().unwrap_or("cmd");
        if exit_code == 0 {
            return pass_output(self.name(), exit_code, format!("PASS ({cmd_name}): exit 0"));
        }

        let mut lines: Vec<String> = raw_output
            .lines()
            .map(str::trim)
            .filter(|l| {
                !l.is_empty()
                    && (l.to_ascii_lowercase().contains("error")
                        || l.to_ascii_lowercase().contains("failed")
                        || l.to_ascii_lowercase().contains("panic"))
            })
            .map(str::to_owned)
            .collect();

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
            format!("FAIL ({cmd_name}): exit {exit_code} ({total} lines)"),
            lines,
            total,
        )
    }
}
