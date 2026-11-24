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

/// Checks `repo_root/node_modules/.bin`, `repo_root/vendor/bin`, then `bin_in_path(name)`.
pub fn resolve_local_or_path_bin(repo_root: &Path, name: &str) -> Option<PathBuf> {
    let trimmed = name.trim();
    let local_node_bin = repo_root.join("node_modules").join(".bin");
    if let Some(local) = find_bin_in_dir(&local_node_bin, trimmed) {
        return Some(local);
    }
    let local_vendor_bin = repo_root.join("vendor").join("bin");
    if let Some(local) = find_bin_in_dir(&local_vendor_bin, trimmed) {
        return Some(local);
    }
    bin_in_path(trimmed)
}

/// Returns the PHP CLI script `repo_root/vendor/bin/<name>` (without `.bat`) if present.
pub fn resolve_php_vendor_script(repo_root: &Path, name: &str) -> Option<PathBuf> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let script = repo_root.join("vendor").join("bin").join(trimmed);
    script.is_file().then_some(script)
}

/// Resolves the PHP runtime binary: `PHP_BINARY`, `PATH`, then common Windows XAMPP/Herd/Laragon paths.
pub fn resolve_php_bin() -> Option<PathBuf> {
    if let Some(env_php) = std::env::var_os("PHP_BINARY") {
        let candidate = PathBuf::from(env_php);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if let Some(in_path) = bin_in_path("php") {
        return Some(in_path);
    }
    #[cfg(windows)]
    {
        if let Some(found) = find_windows_php_bin() {
            return Some(found);
        }
    }
    None
}

#[cfg(windows)]
fn find_windows_php_bin() -> Option<PathBuf> {
    let mut fixed = vec![PathBuf::from(r"C:\xampp\php\php.exe")];
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        let base = PathBuf::from(profile);
        fixed.push(base.join(r".config\herd\bin\php.exe"));
        fixed.push(base.join(r".config\herd-lite\bin\php.exe"));
    }
    for cand in fixed {
        if cand.is_file() {
            return Some(cand);
        }
    }
    let laragon_php = Path::new(r"C:\laragon\bin\php");
    if let Ok(entries) = std::fs::read_dir(laragon_php) {
        let mut versions: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path().join("php.exe"))
            .filter(|p| p.is_file())
            .collect();
        versions.sort();
        if let Some(latest) = versions.pop() {
            return Some(latest);
        }
    }
    None
}

/// Resolves the Godot 4 CLI binary: `GODOT_BIN`, `PATH`, then common Windows/OS install paths.
pub fn resolve_godot_bin() -> Option<PathBuf> {
    if let Some(env_bin) = std::env::var_os("GODOT_BIN") {
        let candidate = PathBuf::from(env_bin);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    for name in ["godot4", "godot", "godot4_console", "godot_console"] {
        if let Some(found) = bin_in_path(name) {
            return Some(found);
        }
    }
    #[cfg(windows)]
    {
        if let Some(found) = find_windows_godot_bin() {
            return Some(found);
        }
    }
    None
}

#[cfg(windows)]
fn find_windows_godot_bin() -> Option<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join(r"Microsoft\WinGet\Links"));
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        dirs.push(PathBuf::from(profile).join(r"scoop\shims"));
    }
    for dir in dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut cands: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| {
                                let l = n.to_ascii_lowercase();
                                l.starts_with("godot") && l.ends_with(".exe")
                            })
                })
                .collect();
            cands.sort();
            if let Some(best) = cands
                .iter()
                .find(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.to_ascii_lowercase().contains("console"))
                })
                .or_else(|| cands.first())
            {
                return Some(best.clone());
            }
        }
    }
    None
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
