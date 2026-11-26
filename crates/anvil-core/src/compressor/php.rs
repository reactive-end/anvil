//! PHPStan/Larastan, Psalm, PHPUnit, Pest, Pint, and `php -l` log parser.

use std::sync::OnceLock;

use regex::Regex;

use crate::compressor::{fail_output, pass_output};
use crate::traits::{CompressedOutput, OutputParser};

pub struct PhpParser;

impl OutputParser for PhpParser {
    fn name(&self) -> &'static str {
        "php"
    }

    fn matches(&self, command: &str) -> bool {
        let lower = command.to_ascii_lowercase();
        lower.contains("phpstan")
            || lower.contains("larastan")
            || lower.contains("psalm")
            || lower.contains("phpunit")
            || lower.contains("pest")
            || lower.contains("pint")
            || lower.contains("php-cs-fixer")
            || lower.contains("artisan test")
            || lower.contains("php -l")
    }

    fn parse(&self, _command: &str, exit_code: i32, raw_output: &str) -> CompressedOutput {
        if exit_code == 0 {
            return pass_output(self.name(), exit_code, "PASS (php): checks passed".to_owned());
        }

        let mut lines = collect_php_diagnostic_lines(raw_output);
        if lines.is_empty() {
            lines = raw_output
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
        }
        let total = lines.len();
        fail_output(self.name(), exit_code, format!("FAIL (php): {total} issue(s)"), lines, total)
    }
}

fn collect_php_diagnostic_lines(raw_output: &str) -> Vec<String> {
    static FILE_LINE_RE: OnceLock<Regex> = OnceLock::new();
    static PHP_SYNTAX_RE: OnceLock<Regex> = OnceLock::new();
    let file_line_re = FILE_LINE_RE.get_or_init(|| {
        Regex::new(r"^(.+?\.php):(\d+)(?::\d+)?\s*:?\s*(.+)$").expect("valid php file:line regex")
    });
    let syntax_re = PHP_SYNTAX_RE.get_or_init(|| {
        Regex::new(
            r"(?i)(Parse error|Fatal error|Syntax error):\s*(.+?)\s+in\s+(.+?\.php)\s+on\s+line\s+(\d+)",
        )
        .expect("valid php syntax regex")
    });

    let mut lines = Vec::new();
    for raw_line in raw_output.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(caps) = syntax_re.captures(trimmed) {
            let kind = caps.get(1).map_or("Error", |m| m.as_str());
            let msg = caps.get(2).map_or("", |m| m.as_str());
            let file = caps.get(3).map_or("", |m| m.as_str());
            let line = caps.get(4).map_or("0", |m| m.as_str());
            lines.push(format!("{file}:{line}: {kind}: {msg}"));
        } else if let Some(caps) = file_line_re.captures(trimmed) {
            let file = caps.get(1).map_or("", |m| m.as_str());
            let line = caps.get(2).map_or("0", |m| m.as_str());
            let msg = caps.get(3).map_or("", |m| m.as_str()).trim();
            lines.push(format!("{file}:{line}: {msg}"));
        } else if is_important_php_diagnostic(trimmed) {
            lines.push(trimmed.to_owned());
        }
    }
    lines
}

fn is_important_php_diagnostic(line: &str) -> bool {
    line.starts_with("FAIL ")
        || line.starts_with("FAILED ")
        || line.starts_with("FAILED!")
        || line.starts_with("ERRORS!")
        || line.starts_with("⨯")
        || line.starts_with("× ")
        || line.starts_with("ERROR:")
        || line.starts_with("Failed asserting")
        || line.starts_with("Expected ")
        || line.starts_with("Tests:")
        || line.starts_with("[ERROR]")
        || line.contains(") ")
            && line.chars().next().is_some_and(|c| c.is_ascii_digit())
            && line.contains("::")
}
