//! Godot 4 headless CLI (`--check-only`, `--import`, GUT/gdlint) log parser.

use crate::compressor::{fail_output, pass_output};
use crate::traits::{CompressedOutput, OutputParser};

pub struct GodotParser;

impl OutputParser for GodotParser {
    fn name(&self) -> &'static str {
        "godot"
    }

    fn matches(&self, command: &str) -> bool {
        let lower = command.to_ascii_lowercase();
        lower.contains("godot") || lower.contains("gdlint")
    }

    fn parse(&self, _command: &str, exit_code: i32, raw_output: &str) -> CompressedOutput {
        let diag_lines = collect_godot_diagnostic_lines(raw_output);
        if exit_code == 0 && diag_lines.is_empty() {
            return pass_output(self.name(), exit_code, "PASS (godot): checks passed".to_owned());
        }

        let lines = if diag_lines.is_empty() {
            raw_output.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect()
        } else {
            diag_lines
        };
        let total = lines.len();
        fail_output(
            self.name(),
            if exit_code == 0 { 1 } else { exit_code },
            format!("FAIL (godot): {total} issue(s)"),
            lines,
            total,
        )
    }
}

fn collect_godot_diagnostic_lines(raw_output: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut keep_next_at = false;
    for raw_line in raw_output.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            keep_next_at = false;
            continue;
        }
        let is_diag = trimmed.starts_with("SCRIPT ERROR:")
            || trimmed.starts_with("Parse Error:")
            || trimmed.starts_with("USER ERROR:")
            || trimmed.starts_with("ERROR:")
            || trimmed.starts_with("FAIL ")
            || is_gdlint_error(trimmed);
        if is_diag {
            lines.push(trimmed.to_owned());
            keep_next_at = true;
        } else if keep_next_at && (trimmed.starts_with("at:") || trimmed.contains("res://")) {
            lines.push(format!("  {trimmed}"));
            keep_next_at = false;
        } else {
            keep_next_at = false;
        }
    }
    lines
}

fn is_gdlint_error(line: &str) -> bool {
    line.contains(".gd:") && line.contains(": Error:")
}
