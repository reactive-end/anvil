//! Classifies recursive-deletion targets as routine cleanup or destructive.

use std::path::Path;

const HOME_PREFIXES: &[&str] = &[
    "~", "$home", "${home}", "%userprofile%", "%homepath%", "$env:userprofile", "$env:home",
    "%systemroot%", "$env:systemroot", "%systemdrive%", "$env:systemdrive", "%programfiles%",
    "$env:programfiles",
];
const TEMP_PREFIXES: &[&str] = &[
    "/tmp/", "/var/tmp/", "/private/tmp/", "/var/folders/", "/dev/shm/",
];

/// Returns `true` when recursively deleting `raw` would reach the filesystem root, a home
/// directory, the whole working directory, `.git`, or anything outside `root` and temp dirs.
pub fn is_dangerous(root: &Path, raw: &str) -> bool {
    let unified = unify(raw);
    if unified.is_empty() {
        return false;
    }
    if HOME_PREFIXES.iter().any(|p| has_path_prefix(&unified, p)) {
        return true;
    }
    let (absolute, parts) = collapse(strip_contents_glob(&unified));
    if parts.is_empty() || parts.last().is_some_and(|p| p == ".git") {
        return true;
    }
    if !absolute {
        return parts[0] == ".." || matches!(parts.join("/").as_str(), "*" | "**" | ".*");
    }
    let path = format!("/{}", parts.join("/"));
    if is_temp_path(&path) {
        return false;
    }
    let repo = format!(
        "/{}",
        collapse(&unify(&root.to_string_lossy())).1.join("/")
    );
    !path.starts_with(&format!("{}/", repo.trim_end_matches('/')))
}

/// Returns `true` for absolute paths strictly inside a temporary directory.
pub fn is_scratch_path(raw: &str) -> bool {
    let (absolute, parts) = collapse(&unify(raw));
    absolute && is_temp_path(&format!("/{}", parts.join("/")))
}

/// Lowercases and rewrites Windows, MSYS, WSL, and Cygwin paths to a `/c:/dir` form.
fn unify(raw: &str) -> String {
    let mut text = raw.trim().replace('\\', "/").to_ascii_lowercase();
    if let Some(rest) = text.strip_prefix("//?/") {
        text = rest.to_owned();
    }
    let mounted = text
        .strip_prefix("/mnt/")
        .or_else(|| text.strip_prefix("/cygdrive/"))
        .or_else(|| text.strip_prefix('/').filter(|_| cfg!(windows)));
    if let Some(rest) = mounted
        && is_drive_segment(rest)
    {
        return format!("/{}:{}", &rest[..1], &rest[1..]);
    }
    let bytes = text.as_bytes();
    if bytes.get(1) == Some(&b':') && bytes[0].is_ascii_alphabetic() {
        return format!("/{text}");
    }
    text
}

fn is_drive_segment(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    bytes.first().is_some_and(u8::is_ascii_alphabetic) && matches!(bytes.get(1), None | Some(b'/'))
}

fn has_path_prefix(text: &str, prefix: &str) -> bool {
    text.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn strip_contents_glob(text: &str) -> &str {
    let mut cur = text;
    loop {
        let next = ["/**", "/*", "/.*", "/."]
            .iter()
            .find_map(|suffix| cur.strip_suffix(suffix))
            .unwrap_or(cur);
        if next == cur {
            return cur;
        }
        cur = if next.is_empty() { "/" } else { next };
    }
}

fn collapse(text: &str) -> (bool, Vec<String>) {
    let absolute = text.starts_with('/');
    let mut parts: Vec<String> = Vec::new();
    for seg in text.split('/') {
        match seg {
            "" | "." => {}
            ".." if parts.last().is_some_and(|p| p != "..") => {
                parts.pop();
            }
            ".." if absolute => {}
            other => parts.push(other.to_owned()),
        }
    }
    if absolute && parts.len() == 1 && parts[0].ends_with(':') {
        parts.clear();
    }
    (absolute, parts)
}

fn is_temp_path(path: &str) -> bool {
    let system_temp = format!(
        "/{}/",
        collapse(&unify(&std::env::temp_dir().to_string_lossy()))
            .1
            .join("/")
    );
    let under_system_temp =
        system_temp.len() > 2 && path.len() > system_temp.len() && path.starts_with(&system_temp);
    under_system_temp
        || path.contains("/appdata/local/temp/")
        || TEMP_PREFIXES
            .iter()
            .any(|p| path.len() > p.len() && path.starts_with(p))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::is_dangerous;

    #[test]
    fn classifies_deletion_targets() {
        let root = Path::new("/home/ada/repo");
        for bad in [
            "/", "/*", "/.", "~", "~/x", "$HOME", "${HOME}/a", ".", "./", "./*", "*", "..", "../x",
            "a/../..", "/etc", "/home/ada", "/home/ada/repo", "/home/ada/repository", "C:\\",
            "C:\\Users", ".git", "sub/.git", "/tmp", "$env:USERPROFILE\\x", "a/..",
        ] {
            assert!(is_dangerous(root, bad), "{bad} should be dangerous");
        }
        for ok in [
            "src", "./target", "node_modules", "dist/*", "/home/ada/repo/target", "/tmp/build",
            "a/../b", "/home/ada/repo/sub/*", "", "$BUILD_DIR",
        ] {
            assert!(!is_dangerous(root, ok), "{ok} should be routine");
        }
    }
}
