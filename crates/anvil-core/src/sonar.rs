//! Opt-in SonarQube integration: `sonar-project.properties` parsing, local Tier 0 complexity
//! & duplication checks, and SonarQube Web API client (`qualitygates` & `issues`).

use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use regex::Regex;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::traits::{Diagnostic, Severity};

/// Parsed fields from `sonar-project.properties`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SonarProperties {
    pub project_key: Option<String>,
    pub host_url: Option<String>,
    pub sources: Option<String>,
    pub exclusions: Option<String>,
}

/// One condition evaluated by a SonarQube Quality Gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateCondition {
    pub status: String,
    pub metric_key: String,
    pub comparator: String,
    pub error_threshold: String,
    pub actual_value: String,
}

/// Summary of `/api/qualitygates/project_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityGateReport {
    pub status: String,
    pub conditions: Vec<GateCondition>,
}

/// Compact issue item returned from `/api/issues/search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SonarIssue {
    pub key: String,
    pub rule: String,
    pub severity: String,
    pub component: String,
    pub line: u32,
    pub message: String,
}

/// Reads and parses `<dir>/sonar-project.properties` when present.
pub fn parse_sonar_properties(dir: &Path) -> Option<SonarProperties> {
    let raw = fs::read_to_string(dir.join("sonar-project.properties")).ok()?;
    let mut props = SonarProperties::default();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
            continue;
        }
        let Some((k, v)) = trimmed.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let val = v.trim();
        if val.is_empty() {
            continue;
        }
        match key {
            "sonar.projectKey" => props.project_key = Some(val.to_owned()),
            "sonar.host.url" => props.host_url = Some(val.trim_end_matches('/').to_owned()),
            "sonar.sources" => props.sources = Some(val.to_owned()),
            "sonar.exclusions" => props.exclusions = Some(val.to_owned()),
            _ => {}
        }
    }
    Some(props)
}

/// Local Tier 0 approximation of Sonar S3776 cognitive complexity per function/method.
pub fn check_cognitive_complexity(
    rel_path: &str,
    source: &str,
    max_complexity: usize,
) -> Vec<Diagnostic> {
    if !is_code_file(rel_path) {
        return Vec::new();
    }
    let lines: Vec<&str> = source.lines().collect();
    let is_indent_lang = rel_path.ends_with(".py") || rel_path.ends_with(".gd");
    let is_rust = rel_path.ends_with(".rs");
    let blocks = collect_fn_blocks(&lines, is_indent_lang, is_rust);
    let mut diags = Vec::new();
    for (name, start_idx, end_idx) in blocks {
        if has_allow_comment(&lines, start_idx, "sonar:cognitive-complexity") {
            continue;
        }
        let score = score_block_complexity(&lines[start_idx..=end_idx], is_indent_lang, is_rust);
        if score > max_complexity {
            diags.push(Diagnostic {
                rule: "sonar:cognitive-complexity".to_owned(),
                severity: Severity::Error,
                file: rel_path.to_owned(),
                line: u32::try_from(start_idx + 1).unwrap_or(1),
                column: 1,
                message: format!(
                    "function `{name}` cognitive complexity {score} exceeds Sonar limit {max_complexity}"
                ),
                suggestion: Some("Extract helper functions or replace deep nesting with early returns.".to_owned()),
            });
        }
    }
    diags
}

/// Local Tier 0 New-Code duplication detector across changed files (`min_lines` normalized window).
pub fn check_new_code_duplication(
    files: &[(String, String)],
    min_lines: usize,
) -> Vec<Diagnostic> {
    let window = min_lines.max(4);
    let mut seen: FxHashMap<blake3::Hash, (String, usize)> = FxHashMap::default();
    let mut diags = Vec::new();

    for (rel, content) in files {
        if !is_code_file(rel) {
            continue;
        }
        let raw_lines: Vec<&str> = content.lines().collect();
        let norm = collect_normalized_lines(&raw_lines);
        if norm.len() < window {
            continue;
        }
        detect_file_duplicates(rel, &raw_lines, &norm, window, &mut seen, &mut diags);
    }
    diags
}

fn detect_file_duplicates(
    rel: &str,
    raw_lines: &[&str],
    norm: &[(usize, String)],
    window: usize,
    seen: &mut FxHashMap<blake3::Hash, (String, usize)>,
    diags: &mut Vec<Diagnostic>,
) {
    let mut last_flagged_end = 0usize;
    for idx in 0..=norm.len() - window {
        let first_line_idx = norm[idx].0;
        if first_line_idx < last_flagged_end
            || has_allow_comment(raw_lines, first_line_idx, "sonar:duplication")
        {
            continue;
        }
        let hash = hash_window(&norm[idx..idx + window]);
        match seen.get(&hash) {
            Some((orig_file, orig_line))
                if orig_file != rel || first_line_idx + 1 >= orig_line + window =>
            {
                diags.push(Diagnostic {
                    rule: "sonar:duplication".to_owned(),
                    severity: Severity::Warning,
                    file: rel.to_owned(),
                    line: u32::try_from(first_line_idx + 1).unwrap_or(1),
                    column: 1,
                    message: format!(
                        "{window}+ normalized lines duplicate `{orig_file}:{orig_line}`"
                    ),
                    suggestion: Some(
                        "Extract a shared helper instead of duplicating logic across files."
                            .to_owned(),
                    ),
                });
                last_flagged_end = norm[idx + window - 1].0 + 1;
            }
            Some(_) => {}
            None => {
                seen.insert(hash, (rel.to_owned(), first_line_idx + 1));
            }
        }
    }
}

/// Fetches Quality Gate status from a SonarQube Server/Cloud instance.
pub fn fetch_quality_gate(
    host_url: &str,
    project_key: &str,
    branch: Option<&str>,
    token: Option<&str>,
) -> Result<QualityGateReport> {
    let base = host_url.trim_end_matches('/');
    let mut url = format!(
        "{base}/api/qualitygates/project_status?projectKey={}",
        percent_encode(project_key)
    );
    if let Some(b) = branch.filter(|s| !s.is_empty()) {
        url.push_str("&branch=");
        url.push_str(&percent_encode(b));
    }
    let body = sonar_get_json(&url, token)?;
    let status = body["projectStatus"]["status"]
        .as_str()
        .unwrap_or("UNKNOWN")
        .to_owned();
    let conditions = body["projectStatus"]["conditions"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|c| GateCondition {
                    status: c["status"].as_str().unwrap_or("").to_owned(),
                    metric_key: c["metricKey"].as_str().unwrap_or("").to_owned(),
                    comparator: c["comparator"].as_str().unwrap_or("").to_owned(),
                    error_threshold: c["errorThreshold"].as_str().unwrap_or("").to_owned(),
                    actual_value: c["actualValue"].as_str().unwrap_or("").to_owned(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(QualityGateReport { status, conditions })
}

/// Fetches unresolved New Code issues from a SonarQube Server/Cloud instance.
pub fn fetch_new_code_issues(
    host_url: &str,
    project_key: &str,
    branch: Option<&str>,
    token: Option<&str>,
    limit: usize,
) -> Result<Vec<SonarIssue>> {
    let base = host_url.trim_end_matches('/');
    let ps = limit.clamp(1, 100);
    let mut url = format!(
        "{base}/api/issues/search?componentKeys={}&resolved=false&inNewCodePeriod=true&ps={ps}",
        percent_encode(project_key)
    );
    if let Some(b) = branch.filter(|s| !s.is_empty()) {
        url.push_str("&branch=");
        url.push_str(&percent_encode(b));
    }
    let body = sonar_get_json(&url, token)?;
    let issues = body["issues"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|item| SonarIssue {
                    key: item["key"].as_str().unwrap_or("").to_owned(),
                    rule: item["rule"].as_str().unwrap_or("").to_owned(),
                    severity: item["severity"].as_str().unwrap_or("").to_owned(),
                    component: item["component"].as_str().unwrap_or("").to_owned(),
                    line: item["line"].as_u64().and_then(|v| u32::try_from(v).ok()).unwrap_or(0),
                    message: item["message"].as_str().unwrap_or("").to_owned(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(issues)
}

fn sonar_get_json(url: &str, token: Option<&str>) -> Result<Value> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build();
    let mut req = agent.get(url);
    if let Some(tok) = token.filter(|t| !t.is_empty()) {
        let encoded = base64_encode(format!("{tok}:").as_bytes());
        req = req.set("Authorization", &format!("Basic {encoded}"));
    }
    let resp = req
        .call()
        .with_context(|| format!("SonarQube request failed: {url}"))?;
    let raw = resp
        .into_string()
        .context("failed to read SonarQube response body")?;
    let val: Value =
        serde_json::from_str(&raw).context("failed to parse SonarQube JSON response")?;
    Ok(val)
}

fn collect_fn_blocks(
    lines: &[&str],
    is_indent_lang: bool,
    is_rust: bool,
) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx < lines.len() {
        let Some(name) = match_fn_header(lines[idx]) else {
            idx += 1;
            continue;
        };
        let end_idx = if is_indent_lang {
            find_indent_block_end(lines, idx)
        } else {
            find_brace_block_end(lines, idx, is_rust)
        };
        if end_idx > idx {
            out.push((name, idx, end_idx));
            idx = end_idx + 1;
        } else {
            idx += 1;
        }
    }
    out
}

fn match_fn_header(line: &str) -> Option<String> {
    static FN_RE: OnceLock<Regex> = OnceLock::new();
    static ARROW_RE: OnceLock<Regex> = OnceLock::new();
    static METHOD_RE: OnceLock<Regex> = OnceLock::new();
    let fn_re = FN_RE.get_or_init(|| {
        Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+|export\s+|private\s+|protected\s+|public\s+|static\s+|async\s+|override\s+)*(?:fn|function|fun|def|func)\s+([A-Za-z_][A-Za-z0-9_]*)")
            .expect("valid fn regex")
    });
    if let Some(name) = fn_re.captures(line).and_then(|c| c.get(1)) {
        return Some(name.as_str().to_owned());
    }
    let arrow_re = ARROW_RE.get_or_init(|| {
        Regex::new(r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?::[^=]+)?=\s*(?:async\s+)?(?:\([^)]*\)|[A-Za-z_][A-Za-z0-9_]*)\s*(?::[^=]+)?=>")
            .expect("valid arrow regex")
    });
    if let Some(name) = arrow_re.captures(line).and_then(|c| c.get(1)) {
        return Some(name.as_str().to_owned());
    }
    let method_re = METHOD_RE.get_or_init(|| {
        Regex::new(r"^\s*(?:public|private|protected|static|async|override)\s+(?:(?:public|private|protected|static|async|override)\s+)*([A-Za-z_][A-Za-z0-9_]*)\s*\([^;]*\)\s*(?::[^{;]+)?\{")
            .expect("valid method regex")
    });
    let cand = method_re.captures(line).and_then(|c| c.get(1))?.as_str();
    (!matches!(cand, "if" | "for" | "while" | "switch" | "catch" | "else" | "match" | "return"))
        .then(|| cand.to_owned())
}

fn find_brace_block_end(lines: &[&str], start: usize, is_rust: bool) -> usize {
    let mut depth = 0i32;
    let mut opened = false;
    let mut in_block_comment = false;
    for (offset, line) in lines[start..].iter().enumerate() {
        let code = strip_strings_and_comments(line, &mut in_block_comment, false, is_rust);
        for ch in code.chars() {
            if ch == '{' {
                depth += 1;
                opened = true;
            } else if ch == '}' {
                depth -= 1;
            }
        }
        if opened && depth <= 0 {
            return start + offset;
        }
    }
    start
}

fn find_indent_block_end(lines: &[&str], start: usize) -> usize {
    let base_indent = leading_indent_cols(lines[start]);
    let mut last = start;
    for (offset, line) in lines[start + 1..].iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = leading_indent_cols(line);
        if indent <= base_indent {
            return last;
        }
        last = start + 1 + offset;
    }
    last
}

fn score_block_complexity(lines: &[&str], is_indent_lang: bool, is_rust: bool) -> usize {
    if is_indent_lang {
        return score_indent_complexity(lines);
    }
    score_brace_complexity(lines, is_rust)
}

fn score_brace_complexity(lines: &[&str], is_rust: bool) -> usize {
    let mut total = 0usize;
    let mut brace_depth = 0i32;
    let mut nesting_depths: Vec<i32> = Vec::new();
    let mut in_block_comment = false;
    for line in lines.iter().skip(1) {
        let code = strip_strings_and_comments(line, &mut in_block_comment, false, is_rust);
        let t = code.trim();
        if t.is_empty() {
            continue;
        }
        let nesting = nesting_depths.len();
        let has_branch = has_branch_keyword(t);
        if has_branch {
            total += 1 + nesting;
        }
        total += count_logical_ops(t);
        if has_ternary_op(t, is_rust) {
            total += 1 + nesting;
        }
        brace_depth += count_char(t, '{') - count_char(t, '}');
        while nesting_depths.last().is_some_and(|&d| brace_depth < d) {
            nesting_depths.pop();
        }
        if has_branch && t.contains('{') {
            nesting_depths.push(brace_depth);
        }
    }
    total
}

fn has_ternary_op(trimmed: &str, is_rust: bool) -> bool {
    if is_rust {
        return false;
    }
    let bytes = trimmed.as_bytes();
    let Some(q_idx) = bytes.iter().enumerate().position(|(i, &b)| {
        if b != b'?' {
            return false;
        }
        let prev = i.checked_sub(1).and_then(|p| bytes.get(p).copied());
        let next = bytes.get(i + 1).copied();
        prev != Some(b'?') && !matches!(next, Some(b'?' | b'.' | b':'))
    }) else {
        return false;
    };
    let tail = &bytes[q_idx + 1..];
    tail.iter().enumerate().any(|(i, &b)| {
        b == b':'
            && i.checked_sub(1).and_then(|p| tail.get(p).copied()) != Some(b':')
            && tail.get(i + 1).copied() != Some(b':')
    })
}

fn score_indent_complexity(lines: &[&str]) -> usize {
    let mut total = 0usize;
    let mut indent_stack: Vec<usize> = Vec::new();
    let mut dummy_comment = false;
    for line in lines.iter().skip(1) {
        let code = strip_strings_and_comments(line, &mut dummy_comment, true, false);
        let t = code.trim();
        if t.is_empty() {
            continue;
        }
        let cur_indent = leading_indent_cols(line);
        while indent_stack.last().is_some_and(|&d| cur_indent <= d) {
            indent_stack.pop();
        }
        let nesting = indent_stack.len();
        let has_branch = has_branch_keyword(t) || t.starts_with("except ") || t.starts_with("except:");
        if has_branch {
            total += 1 + nesting;
            indent_stack.push(cur_indent);
        }
        total += count_logical_ops(t) + count_word_ops(t, "and") + count_word_ops(t, "or");
    }
    total
}

fn strip_strings_and_comments(
    line: &str,
    in_block_comment: &mut bool,
    hash_comments: bool,
    is_rust: bool,
) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut i = 0usize;
    let mut quote: Option<char> = None;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if *in_block_comment {
            let ended = c == '*' && next == Some('/');
            *in_block_comment = !ended;
            i += if ended { 2 } else { 1 };
            continue;
        }
        if let Some(q) = quote {
            i += advance_quoted(c, q, &mut quote);
            continue;
        }
        if (c == '/' && next == Some('/')) || (hash_comments && c == '#') {
            break;
        }
        if c == '/' && next == Some('*') {
            *in_block_comment = true;
            i += 2;
            continue;
        }
        if c == '\'' && is_rust {
            if let Some(next_i) = skip_rust_char_literal(&chars, i) {
                out.push(' ');
                i = next_i;
                continue;
            }
            out.push(c);
            i += 1;
            continue;
        }
        if matches!(c, '"' | '\'' | '`') {
            quote = Some(c);
            out.push(' ');
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

fn advance_quoted(c: char, active: char, quote: &mut Option<char>) -> usize {
    if c == '\\' {
        return 2;
    }
    if c == active {
        *quote = None;
    }
    1
}

fn skip_rust_char_literal(chars: &[char], start: usize) -> Option<usize> {
    if chars.get(start + 1) == Some(&'\\') {
        let limit = (start + 14).min(chars.len());
        for (idx, &ch) in chars.iter().enumerate().take(limit).skip(start + 3) {
            if ch == '\'' {
                return Some(idx + 1);
            }
            if matches!(ch, '\n' | '\r' | ' ' | ';') {
                break;
            }
        }
        return None;
    }
    (chars.get(start + 2) == Some(&'\'') && chars.get(start + 1) != Some(&'\''))
        .then_some(start + 3)
}

fn leading_indent_cols(line: &str) -> usize {
    line.chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

fn count_word_ops(line: &str, kw: &str) -> usize {
    line.match_indices(kw)
        .filter(|&(idx, _)| {
            let prev_ok = line[..idx]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
            let next_ok = line[idx + kw.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
            prev_ok && next_ok
        })
        .count()
}

fn has_branch_keyword(trimmed: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "if ", "if(", "else if", "elif ", "for ", "for(", "while ", "while(", "match ", "switch ",
        "switch(", "catch ", "catch(", "} else if",
    ];
    PREFIXES.iter().any(|p| trimmed.starts_with(p))
}

fn count_logical_ops(line: &str) -> usize {
    line.matches("&&").count() + line.matches("||").count()
}

fn count_char(s: &str, ch: char) -> i32 {
    i32::try_from(s.chars().filter(|&c| c == ch).count()).unwrap_or(0)
}

fn collect_normalized_lines(lines: &[&str]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with('#')
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
            || matches!(trimmed, "{" | "}" | "};" | ");" | ")")
        {
            continue;
        }
        let compact = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
        out.push((idx, compact));
    }
    out
}

fn hash_window(slice: &[(usize, String)]) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    for (_, line) in slice {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize()
}

fn has_allow_comment(lines: &[&str], idx: usize, rule: &str) -> bool {
    let marker = format!("anvil-allow({rule}):");
    let start = idx.saturating_sub(3);
    lines[start..=idx].iter().any(|line| {
        let trimmed = line.trim();
        let is_comment = trimmed.starts_with("//")
            || trimmed.starts_with('#')
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*');
        if !is_comment {
            return false;
        }
        let Some((_, rest)) = trimmed.split_once(&marker) else {
            return false;
        };
        let reason = rest.trim().trim_end_matches("*/").trim();
        reason.chars().filter(|c| !c.is_whitespace()).count() >= 3
    })
}

fn is_code_file(rel: &str) -> bool {
    const EXTS: &[&str] = &[
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".php", ".py", ".kt", ".java", ".dart", ".gd",
        ".go", ".cs",
    ];
    let lower = rel.to_ascii_lowercase();
    EXTS.iter().any(|ext| lower.ends_with(ext))
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(char::from(TABLE[((n >> 18) & 63) as usize]));
        out.push(char::from(TABLE[((n >> 12) & 63) as usize]));
        out.push(if chunk.len() > 1 {
            char::from(TABLE[((n >> 6) & 63) as usize])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(TABLE[(n & 63) as usize])
        } else {
            '='
        });
    }
    out
}
