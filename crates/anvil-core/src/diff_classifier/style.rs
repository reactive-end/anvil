//! Style-only hunk inspection for JSX/TSX/HTML/CSS diffs.

use std::sync::OnceLock;

use regex::Regex;

/// Returns true if all diff hunks represent purely CSS or `className`/`class`/`style` changes.
pub(super) fn hunks_are_style_only(hunks: &str) -> bool {
    static STYLE_LINE_RE: OnceLock<Regex> = OnceLock::new();
    let re = STYLE_LINE_RE.get_or_init(|| {
        Regex::new(r#"^[+-]\s*(?:className=|class=|style=|\.?[a-zA-Z0-9_-]+\s*:\s*['"]?#[0-9a-fA-F]{3,8}|color:|background|border-color)"#)
            .expect("valid static style regex")
    });

    let mut saw_change = false;
    let mut saw_style_change = false;
    let mut removed: Vec<String> = Vec::new();
    let mut added: Vec<String> = Vec::new();

    for raw_line in hunks.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
            if !flush_change_block(&mut removed, &mut added) {
                return false;
            }
            continue;
        }
        if line.starts_with('-') || line.starts_with('+') {
            saw_change = true;
            if re.is_match(line) {
                saw_style_change = true;
                continue;
            }
            let (skel, stripped) = strip_style_attrs(&line[1..]);
            if stripped {
                saw_style_change = true;
            }
            if !skel.is_empty() {
                if line.starts_with('-') {
                    removed.push(skel);
                } else {
                    added.push(skel);
                }
            }
        } else if !flush_change_block(&mut removed, &mut added) {
            return false;
        }
    }

    saw_change && saw_style_change && flush_change_block(&mut removed, &mut added)
}

fn flush_change_block(removed: &mut Vec<String>, added: &mut Vec<String>) -> bool {
    let ok = normalize_ws(&removed.join(" ")) == normalize_ws(&added.join(" "));
    removed.clear();
    added.clear();
    ok
}

fn strip_style_attrs(line: &str) -> (String, bool) {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    let mut stripped_any = false;

    while i < bytes.len() {
        if let Some(next_i) = try_skip_style_attr(line, i) {
            stripped_any = true;
            i = next_i;
            continue;
        }
        if let Some(ch) = line[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    (normalize_ws(&out), stripped_any)
}

fn try_skip_style_attr(line: &str, idx: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    if idx > 0 && !bytes[idx - 1].is_ascii_whitespace() {
        return None;
    }
    let rest = &line[idx..];
    let attr_len = if rest.starts_with("className=") {
        "className=".len()
    } else if rest.starts_with("class=") {
        "class=".len()
    } else if rest.starts_with("style=") {
        "style=".len()
    } else {
        return None;
    };

    let mut pos = idx + attr_len;
    while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }
    let quote_or_brace = *bytes.get(pos)?;
    match quote_or_brace {
        b'"' | b'\'' => skip_quoted(bytes, pos + 1, quote_or_brace),
        b'{' => skip_braced(bytes, pos + 1),
        _ => None,
    }
}

fn skip_quoted(bytes: &[u8], mut pos: usize, delim: u8) -> Option<usize> {
    let mut escaped = false;
    while pos < bytes.len() {
        let b = bytes[pos];
        if escaped {
            escaped = false;
        } else if b == b'\\' {
            escaped = true;
        } else if b == delim {
            return Some(pos + 1);
        }
        pos += 1;
    }
    None
}

fn skip_braced(bytes: &[u8], mut pos: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut in_quote: Option<u8> = None;
    let mut escaped = false;
    while pos < bytes.len() {
        let b = bytes[pos];
        if let Some(q) = in_quote {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == q {
                in_quote = None;
            }
        } else {
            match b {
                b'"' | b'\'' | b'`' => in_quote = Some(b),
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(pos + 1);
                    }
                }
                _ => {}
            }
        }
        pos += 1;
    }
    None
}

fn normalize_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for ch in s.trim().chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
        } else {
            if (ch == '>' || ch == '/') && out.ends_with(' ') {
                out.pop();
            }
            out.push(ch);
            prev_ws = false;
        }
    }
    out.trim().to_owned()
}
