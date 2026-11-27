//! Finds the SQLite file an app uses by reading only the path variables of its `.env*` files.

use std::fs;
use std::path::Path;

/// Variables that hold a database file path, in order of preference.
const PATH_VARS: [&str; 5] =
    ["DB_PATH", "DATABASE_PATH", "SQLITE_PATH", "DB_FILE", "DATABASE_FILE"];

/// Variables that hold a connection URL (`sqlite:./dev.db`, `file:./dev.db`).
const URL_VARS: [&str; 2] = ["DATABASE_URL", "DB_URL"];

const ENV_FILES: [&str; 4] = [".env", ".env.local", ".env.development", ".env.example"];
const FILE_EXTENSIONS: [&str; 5] = [".sqlite", ".sqlite3", ".db", ".db3", ".s3db"];
const SKIPPED_DIRS: [&str; 5] = ["node_modules", "target", "dist", "build", "vendor"];

/// Where a path was found, for the message that tells the user what was used.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Found {
    /// The path, relative to the repository root with forward slashes (absolute stays absolute).
    pub path: String,
    pub variable: String,
    /// The env file, relative to the repository root.
    pub file: String,
}

/// The first SQLite path in `.env*` files of the root or its direct subfolders. Nothing but the
/// path variables is read; no other value leaves this function.
pub(super) fn find_sqlite_path(root: &Path) -> Option<Found> {
    let mut dirs = vec![(root.to_path_buf(), String::new())];
    let mut subdirs: Vec<_> = fs::read_dir(root)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.') && !SKIPPED_DIRS.contains(&name.as_str()))
        .collect();
    subdirs.sort();
    dirs.extend(subdirs.into_iter().map(|name| (root.join(&name), format!("{name}/"))));
    ENV_FILES.iter().find_map(|file| {
        dirs.iter().find_map(|(dir, prefix)| {
            let text = fs::read_to_string(dir.join(file)).ok()?;
            let (variable, value) = pick_path(&text)?;
            Some(Found { path: anchor(&value, prefix), variable, file: format!("{prefix}{file}") })
        })
    })
}

/// The first path-like variable of `text`, with its value.
fn pick_path(text: &str) -> Option<(String, String)> {
    let vars: Vec<(String, String)> = text.lines().filter_map(parse_line).collect();
    let lookup = |name: &str| vars.iter().find(|(key, _)| key == name).map(|(_, v)| v.as_str());
    for name in PATH_VARS {
        if let Some(value) = lookup(name).filter(|v| has_db_extension(v)) {
            return Some((name.to_owned(), value.to_owned()));
        }
    }
    URL_VARS.iter().find_map(|name| {
        let path = path_from_url(lookup(name)?)?;
        Some(((*name).to_owned(), path))
    })
}

fn parse_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").unwrap_or(line);
    let (key, value) = line.split_once('=')?;
    let value = value.trim();
    let value = match value.chars().next() {
        Some(quote @ ('"' | '\'')) => value[1..].split(quote).next().unwrap_or_default(),
        _ => value.split(" #").next().unwrap_or_default().trim(),
    };
    Some((key.trim().to_owned(), value.to_owned()))
}

fn has_db_extension(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    FILE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

fn path_from_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("sqlite://").or_else(|| url.strip_prefix("sqlite:"))?;
    let rest = rest.strip_prefix("file:").unwrap_or(rest);
    let path = rest.split('?').next().unwrap_or_default();
    // `sqlite:///abs/path` keeps the leading slash of an absolute path; `sqlite://./x` is relative.
    (!path.is_empty() && path != ":memory:").then(|| path.to_owned())
}

/// Makes `path` relative to the repository root when the env file sat in a subfolder.
fn anchor(path: &str, prefix: &str) -> String {
    let unified = path.replace('\\', "/");
    let is_absolute = unified.starts_with('/') || unified.as_bytes().get(1) == Some(&b':');
    if is_absolute {
        return unified;
    }
    let relative = unified.trim_start_matches("./");
    format!("{prefix}{relative}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        fs::create_dir_all(dir).expect("mkdir");
        fs::write(dir.join(name), text).expect("write");
    }

    #[test]
    fn a_path_variable_in_a_subfolder_becomes_a_root_relative_path() {
        let dir = tempfile::tempdir().expect("tmp");
        write(&dir.path().join("orders-api"), ".env", "PORT=3100\nDB_PATH=data/orders.sqlite\n");
        let found = find_sqlite_path(dir.path()).expect("found");
        assert_eq!(found.path, "orders-api/data/orders.sqlite");
        assert_eq!(found.variable, "DB_PATH");
        assert_eq!(found.file, "orders-api/.env");
    }

    #[test]
    fn urls_quotes_and_comments_are_understood() {
        let dir = tempfile::tempdir().expect("tmp");
        write(
            dir.path(),
            ".env",
            "# DB_PATH=old.sqlite\nDATABASE_URL=\"sqlite:./dev.db?mode=rw\" # local\n",
        );
        let found = find_sqlite_path(dir.path()).expect("found");
        assert_eq!((found.path.as_str(), found.variable.as_str()), ("dev.db", "DATABASE_URL"));
        assert_eq!(pick_path("DATABASE_URL=postgres://u:secret@h/db\nTOKEN=abc"), None);
        assert_eq!(pick_path("DB_PATH=:memory:"), None);
    }

    #[test]
    fn real_env_files_win_over_examples_and_nothing_is_found_without_a_path() {
        let dir = tempfile::tempdir().expect("tmp");
        write(dir.path(), ".env.example", "DB_PATH=example.sqlite\n");
        write(dir.path(), ".env", "DB_PATH=real.sqlite\n");
        assert_eq!(find_sqlite_path(dir.path()).expect("found").path, "real.sqlite");
        let empty = tempfile::tempdir().expect("tmp");
        write(empty.path(), ".env", "API_KEY=hunter2\n");
        assert_eq!(find_sqlite_path(empty.path()), None);
    }

    #[test]
    fn absolute_paths_stay_absolute() {
        assert_eq!(anchor("C:\\data\\app.db", "api/"), "C:/data/app.db");
        assert_eq!(anchor("/var/app.db", "api/"), "/var/app.db");
        assert_eq!(anchor("./app.db", ""), "app.db");
    }
}
