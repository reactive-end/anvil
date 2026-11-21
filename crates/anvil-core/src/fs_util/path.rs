//! Portable binary resolution, author slugging, and path relativization utilities.

use std::path::{Path, PathBuf};

/// Searches `PATH` for `name`, handling Windows `PATHEXT` (`.exe`/`.cmd`/`.bat`) extensions.
pub fn bin_in_path(name: &str) -> Option<PathBuf> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if let Some(found) = find_bin_in_dir(&dir, trimmed) {
            return Some(found);
        }
    }
    None
}

/// Checks `repo_root/node_modules/.bin/<name>[.cmd|.exe]` first, then `bin_in_path(name)`.
pub fn resolve_local_or_path_bin(repo_root: &Path, name: &str) -> Option<PathBuf> {
    let local_bin_dir = repo_root.join("node_modules").join(".bin");
    if let Some(local) = find_bin_in_dir(&local_bin_dir, name.trim()) {
        return Some(local);
    }
    bin_in_path(name)
}

fn find_bin_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() || !dir.is_dir() {
        return None;
    }
    if cfg!(windows) {
        let lower = name.to_ascii_lowercase();
        let exts = windows_pathext();
        if exts.iter().any(|ext| lower.ends_with(&ext.to_ascii_lowercase())) {
            let candidate = dir.join(name);
            return candidate.is_file().then_some(candidate);
        }
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        let bare = dir.join(name);
        if bare.extension().is_some() && bare.is_file() {
            return Some(bare);
        }
        None
    } else {
        let candidate = dir.join(name);
        is_executable_file(&candidate).then_some(candidate)
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

fn windows_pathext() -> Vec<String> {
    let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
    let mut exts: Vec<String> = raw
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    for fallback in [".EXE", ".CMD", ".BAT"] {
        if !exts.iter().any(|e| e.eq_ignore_ascii_case(fallback)) {
            exts.push(fallback.to_owned());
        }
    }
    exts
}

/// Sanitizes an author string into a portable ASCII slug `[a-zA-Z0-9_-]`, fallback `"unknown"`.
pub fn sanitize_author(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_dash = false;

    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }

    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "unknown".to_owned()
    } else {
        out
    }
}

/// Relativizes `path` against `root` with forward slashes and case-insensitive Windows drive/prefix matching.
pub fn relativize_path(root: &Path, path: &Path) -> String {
    let root_norm = normalize_slashed_path(root, true);
    let path_norm = normalize_slashed_path(path, false);

    let case_insensitive =
        cfg!(windows) || (has_drive_prefix(&root_norm) && has_drive_prefix(&path_norm));
    let matched = if case_insensitive {
        path_norm
            .to_ascii_lowercase()
            .starts_with(&root_norm.to_ascii_lowercase())
    } else {
        path_norm.starts_with(&root_norm)
    };

    if matched && !root_norm.is_empty() {
        let len = root_norm.len();
        let at_boundary = path_norm.len() == len
            || root_norm.ends_with('/')
            || path_norm.as_bytes().get(len) == Some(&b'/');
        if at_boundary {
            return path_norm[len..].trim_start_matches('/').to_owned();
        }
    }

    path_norm
        .strip_prefix("./")
        .unwrap_or(&path_norm)
        .to_owned()
}

fn normalize_slashed_path(path: &Path, trim_trailing_slash: bool) -> String {
    let mut s = path.to_string_lossy().replace('\\', "/");
    if let Some(unc) = s.strip_prefix("//?/UNC/") {
        s = format!("//{unc}");
    } else if let Some(verbatim) = s.strip_prefix("//?/") {
        s = verbatim.to_owned();
    }
    if trim_trailing_slash && s.len() > 1 && !(s.len() == 3 && has_drive_prefix(&s)) {
        while s.ends_with('/') && s.len() > 1 && !(s.len() == 3 && has_drive_prefix(&s)) {
            s.pop();
        }
    }
    s
}

fn has_drive_prefix(slashed: &str) -> bool {
    let b = slashed.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}
