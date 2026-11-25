//! TypeScript compiler (`tsc` / `ngc` / `vue-tsc`) log parser.

use std::sync::OnceLock;

use regex::Regex;

use crate::compressor::{fail_output, pass_output, strip_ansi};
use crate::traits::{CompressedOutput, OutputParser};

pub struct TscParser;

impl OutputParser for TscParser {
    fn name(&self) -> &'static str {
        "tsc"
    }

    fn matches(&self, command: &str) -> bool {
        command_has_tool_token(command, &["tsc", "ngc", "vue-tsc", "tsgo"])
    }

    fn parse(&self, _command: &str, exit_code: i32, raw_output: &str) -> CompressedOutput {
        if exit_code == 0 {
            return pass_output(self.name(), exit_code, "PASS (tsc): 0 type errors".to_owned());
        }

        static TSC_ERR_RE: OnceLock<Regex> = OnceLock::new();
        let re = TSC_ERR_RE.get_or_init(|| {
            Regex::new(r"^(.+?)(?:\((\d+),(\d+)\)|:(\d+):(\d+))\s*[:-]?\s*(error|warning)\s+(TS\d+):\s*(.+)$")
                .expect("valid tsc regex")
        });

        let cleaned = strip_ansi(raw_output);
        let mut lines = Vec::new();
        for line in cleaned.lines() {
            let trimmed = line.trim();
            if let Some(caps) = re.captures(trimmed) {
                let file = caps.get(1).map_or("", |m| m.as_str());
                let ln = caps.get(2).or_else(|| caps.get(4)).map_or("0", |m| m.as_str());
                let col = caps.get(3).or_else(|| caps.get(5)).map_or("0", |m| m.as_str());
                let code = caps.get(7).map_or("TS", |m| m.as_str());
                let msg = caps.get(8).map_or("", |m| m.as_str());
                lines.push(format!("{file}:{ln}:{col} [{code}] {msg}"));
            }
        }

        if lines.is_empty() {
            lines = cleaned
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
            format!("FAIL (tsc): {total} type error(s)"),
            lines,
            total,
        )
    }
}

fn command_has_tool_token(command: &str, tools: &[&str]) -> bool {
    for raw_tok in command.split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')')) {
        let tok = raw_tok.trim_matches(|c| c == '"' || c == '\'');
        if tok.is_empty() {
            continue;
        }
        let base = tok.rsplit(['/', '\\']).next().unwrap_or(tok);
        let lower = base.to_ascii_lowercase();
        let no_ext = lower
            .strip_suffix(".exe")
            .or_else(|| lower.strip_suffix(".cmd"))
            .or_else(|| lower.strip_suffix(".bat"))
            .unwrap_or(&lower);
        let tool_name = no_ext.split('@').next().unwrap_or(no_ext);
        if tools.contains(&tool_name) {
            return true;
        }
    }
    false
}
