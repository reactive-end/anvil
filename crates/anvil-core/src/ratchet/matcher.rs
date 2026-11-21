//! File glob and language target matching for ratchet rules and candidates.

pub(super) fn matches_target(rel_path: &str, file_globs: &[String], languages: &[String]) -> bool {
    let glob_ok = file_globs.is_empty() || file_globs.iter().any(|g| glob_matches(g, rel_path));
    let lang_ok = languages.is_empty() || matches_language(rel_path, languages);
    glob_ok && lang_ok
}

fn matches_language(rel_path: &str, languages: &[String]) -> bool {
    let ext = rel_path
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
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
            _ => false,
        }
    })
}

fn glob_matches(pattern: &str, path: &str) -> bool {
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
            match_segments(&pat[1..], path)
                || (!path.is_empty() && match_segments(pat, &path[1..]))
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
