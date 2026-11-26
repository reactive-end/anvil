//! File glob, language target, comment-context suppression, and repo file scanning for ratchet.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

pub(super) fn matches_target(rel_path: &str, file_globs: &[String], languages: &[String]) -> bool {
    let glob_ok = file_globs.is_empty() || file_globs.iter().any(|g| glob_matches(g, rel_path));
    let lang_ok = languages.is_empty() || matches_language(rel_path, languages);
    glob_ok && lang_ok
}

fn matches_language(rel_path: &str, languages: &[String]) -> bool {
    let ext = rel_path.rsplit('.').next().map(str::to_ascii_lowercase).unwrap_or_default();
    if ext.is_empty() || !rel_path.contains('.') {
        return false;
    }
    languages.iter().any(|lang| {
        let l = lang.trim().to_ascii_lowercase();
        if l == ext {
            return true;
        }
        match l.as_str() {
            "typescript" | "ts" => matches!(ext.as_str(), "ts" | "tsx" | "mts" | "cts"),
            "javascript" | "js" => matches!(ext.as_str(), "js" | "jsx" | "mjs" | "cjs"),
            "python" | "py" => matches!(ext.as_str(), "py" | "pyi"),
            "kotlin" | "kt" => matches!(ext.as_str(), "kt" | "kts"),
            "rust" | "rs" => ext == "rs",
            "dart" => ext == "dart",
            "css" | "scss" => matches!(ext.as_str(), "css" | "scss"),
            "html" => matches!(ext.as_str(), "html" | "htm"),
            "vue" => ext == "vue",
            "svelte" => ext == "svelte",
            "astro" => ext == "astro",
            "php" => matches!(ext.as_str(), "php" | "phtml"),
            "blade" => rel_path.to_ascii_lowercase().ends_with(".blade.php"),
            _ => false,
        }
    })
}

/// Matches a repository-relative path against a `*`/`**` glob.
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    let norm_pat = pattern.replace('\\', "/");
    let norm_pat = norm_pat.strip_prefix("./").unwrap_or(&norm_pat);
    let norm_path = path.strip_prefix("./").unwrap_or(path);

    if !norm_pat.contains('/') {
        let file_name = norm_path.rsplit('/').next().unwrap_or(norm_path);
        return wildcard_match(norm_pat.as_bytes(), file_name.as_bytes());
    }
    let pat_segs: Vec<&str> = norm_pat.split('/').filter(|s| !s.is_empty()).collect();
    let path_segs: Vec<&str> = norm_path.split('/').filter(|s| !s.is_empty()).collect();
    match_segments(&pat_segs, &path_segs)
}

fn match_segments(pat: &[&str], path: &[&str]) -> bool {
    match (pat.first(), path.first()) {
        (None, None) => true,
        (Some(&"**"), _) => {
            match_segments(&pat[1..], path) || (!path.is_empty() && match_segments(pat, &path[1..]))
        }
        (Some(p), Some(s)) => {
            wildcard_match(p.as_bytes(), s.as_bytes()) && match_segments(&pat[1..], &path[1..])
        }
        _ => false,
    }
}

fn wildcard_match(pat: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star_p, mut star_t) = (None, 0usize);
    while t < text.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star_p = Some(p);
            p += 1;
            star_t = t;
        } else if let Some(sp) = star_p {
            p = sp + 1;
            star_t += 1;
            t = star_t;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

pub(super) fn is_suppressed(lines: &[&str], idx: usize, rule_id: &str, rule_name: &str) -> bool {
    let id_marker = format!("anvil-allow({rule_id}):");
    let name_marker = format!("anvil-allow({rule_name}):");
    let check_line = |line: &str| -> bool {
        marker_in_comment(line, &id_marker) || marker_in_comment(line, &name_marker)
    };
    check_line(lines[idx]) || (idx > 0 && check_line(lines[idx - 1]))
}

fn marker_in_comment(line: &str, marker: &str) -> bool {
    let bytes = line.as_bytes();
    let needle = marker.as_bytes();
    let mut i = 0usize;
    let mut in_quote: Option<u8> = None;
    let mut escaped = false;

    while i < bytes.len() {
        if let Some(q) = in_quote {
            if escaped {
                escaped = false;
            } else if bytes[i] == b'\\' {
                escaped = true;
            } else if bytes[i] == q {
                in_quote = None;
            }
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"//") || bytes[i..].starts_with(b"--") || bytes[i] == b'#' {
            return contains_bytes(&bytes[i..], needle);
        }
        if bytes[i..].starts_with(b"/*") {
            let end = bytes[i + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .map_or(bytes.len(), |p| i + 4 + p);
            if contains_bytes(&bytes[i..end], needle) {
                return true;
            }
            i = end;
            continue;
        }
        if bytes[i] == b'"'
            || bytes[i] == b'`'
            || (bytes[i] == b'\'' && has_closing_single_quote(&bytes[i + 1..]))
        {
            in_quote = Some(bytes[i]);
        }
        i += 1;
    }
    false
}

fn has_closing_single_quote(rest: &[u8]) -> bool {
    let mut escaped = false;
    for &b in rest {
        if escaped {
            escaped = false;
        } else if b == b'\\' {
            escaped = true;
        } else if b == b'\'' {
            return true;
        }
    }
    false
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

pub(super) fn is_match_in_comment_or_string(
    rel_path: &str,
    line: &str,
    match_start: usize,
) -> bool {
    let bytes = line.as_bytes();
    let limit = match_start.min(bytes.len());
    let allow_hash = uses_hash_comments(rel_path);
    let allow_dash = uses_dash_comments(rel_path);
    let mut i = 0usize;
    let mut in_quote: Option<u8> = None;
    let mut escaped = false;

    while i < limit {
        if let Some(q) = in_quote {
            if escaped {
                escaped = false;
            } else if bytes[i] == b'\\' {
                escaped = true;
            } else if bytes[i] == q {
                in_quote = None;
            }
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"//")
            || (allow_dash && bytes[i..].starts_with(b"--"))
            || (allow_hash && bytes[i] == b'#')
        {
            return true;
        }
        if bytes[i..].starts_with(b"/*") {
            let Some(p) = bytes[i + 2..].windows(2).position(|w| w == b"*/") else {
                return true;
            };
            let end = i + 4 + p;
            if end > limit {
                return true;
            }
            i = end;
            continue;
        }
        if bytes[i] == b'"'
            || bytes[i] == b'`'
            || (bytes[i] == b'\'' && has_closing_single_quote(&bytes[i + 1..]))
        {
            in_quote = Some(bytes[i]);
        }
        i += 1;
    }
    in_quote.is_some()
}

fn uses_hash_comments(rel_path: &str) -> bool {
    let name = rel_path.rsplit(['/', '\\']).next().unwrap_or(rel_path);
    if matches!(
        name.to_ascii_lowercase().as_str(),
        "dockerfile" | "makefile" | "gemfile" | "rakefile"
    ) {
        return true;
    }
    let ext = rel_path.rsplit('.').next().map(str::to_ascii_lowercase).unwrap_or_default();
    matches!(
        ext.as_str(),
        "py" | "pyi"
            | "sh"
            | "bash"
            | "zsh"
            | "rb"
            | "yml"
            | "yaml"
            | "toml"
            | "gd"
            | "php"
            | "phtml"
            | "pl"
            | "r"
    )
}

fn uses_dash_comments(rel_path: &str) -> bool {
    let ext = rel_path.rsplit('.').next().map(str::to_ascii_lowercase).unwrap_or_default();
    matches!(ext.as_str(), "sql" | "lua" | "hs")
}

pub(super) fn collect_repo_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        bail!("failed to read directory {}", dir.display());
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_symlink() {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if ft.is_dir() {
            if is_ignored_dir(&name_str) {
                continue;
            }
            collect_repo_files(&path, out)?;
        } else if ft.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn is_ignored_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".harness"
            | "target"
            | "node_modules"
            | "dist"
            | "build"
            | ".next"
            | ".gradle"
            | "__pycache__"
            | ".venv"
            | "venv"
    )
}
