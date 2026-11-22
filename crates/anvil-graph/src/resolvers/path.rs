//! Relative path joining and segment normalization helpers for `resolvers`.

/// Joins a base directory and a relative specifier, resolving `.` and `..` segments cleanly.
pub fn normalize_join_rel(base_dir: &str, rel: &str) -> String {
    let clean_rel = rel.replace('\\', "/");
    let combined = if base_dir.is_empty() || base_dir == "." {
        clean_rel
    } else {
        format!("{base_dir}/{clean_rel}")
    };
    normalize_segments(&combined)
}

/// Joins two normalized relative paths without parent-directory traversal unless present.
pub fn join_normalized(base: &str, sub: &str) -> String {
    let clean_base = base.trim_matches('/');
    let clean_sub = sub.trim_start_matches('/');
    if clean_base.is_empty() || clean_base == "." {
        normalize_segments(clean_sub)
    } else if clean_sub.is_empty() {
        normalize_segments(clean_base)
    } else {
        normalize_segments(&format!("{clean_base}/{clean_sub}"))
    }
}

fn normalize_segments(path: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    stack.join("/")
}
