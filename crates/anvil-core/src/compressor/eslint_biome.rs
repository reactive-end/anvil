//! ESLint and Biome log parser.

use std::sync::OnceLock;

use regex::Regex;

use crate::compressor::{fail_output, pass_output};
use crate::traits::{CompressedOutput, OutputParser};

pub struct EslintBiomeParser;

impl OutputParser for EslintBiomeParser {
    fn name(&self) -> &'static str {
        "eslint/biome"
    }

    fn matches(&self, command: &str) -> bool {
        let lower = command.to_ascii_lowercase();
        lower.contains("eslint") || lower.contains("biome")
    }

    fn parse(&self, _command: &str, exit_code: i32, raw_output: &str) -> CompressedOutput {
        if exit_code == 0 {
            return pass_output(
                self.name(),
                exit_code,
                "PASS (eslint/biome): 0 lint issues".to_owned(),
            );
        }

        let lines = extract_findings(raw_output);
        let total = lines.len();
        fail_output(
            self.name(),
            exit_code,
            format!("FAIL (eslint/biome): {total} lint finding(s)"),
            lines,
            total,
        )
    }
}

fn extract_findings(raw_output: &str) -> Vec<String> {
    static ESLINT_ITEM_RE: OnceLock<Regex> = OnceLock::new();
    static BIOME_HDR_RE: OnceLock<Regex> = OnceLock::new();

    let eslint_re = ESLINT_ITEM_RE.get_or_init(|| {
        Regex::new(r"^\s*(\d+:\d+)\s+(error|warning)\s+(.+?)(?:\s{2,}(\S+))?$")
            .expect("valid eslint regex")
    });
    let biome_re = BIOME_HDR_RE.get_or_init(|| {
        Regex::new(r"^(\S+:\d+:\d+)\s+(lint/\S+|format|assist/\S+)").expect("valid biome regex")
    });

    let mut current_file = String::new();
    let mut lines = Vec::new();
    for raw_line in raw_output.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(caps) = biome_re.captures(trimmed) {
            let loc = caps.get(1).map_or("", |m| m.as_str());
            let rule = caps.get(2).map_or("", |m| m.as_str());
            lines.push(format!("{loc} [{rule}]"));
            continue;
        }
        if let Some(caps) = eslint_re.captures(raw_line) {
            let loc = caps.get(1).map_or("0:0", |m| m.as_str());
            let sev = caps.get(2).map_or("error", |m| m.as_str());
            let msg = caps.get(3).map_or("", |m| m.as_str()).trim();
            let rule = caps.get(4).map_or("", |m| m.as_str());
            let prefix = if current_file.is_empty() {
                loc.to_owned()
            } else {
                format!("{current_file}:{loc}")
            };
            lines.push(format!("{prefix} {sev} [{rule}] {msg}"));
            continue;
        }
        if !trimmed.starts_with('✖') && !trimmed.starts_with("Found ") && trimmed.contains('.') {
            current_file = trimmed.to_owned();
        }
    }
    lines
}
