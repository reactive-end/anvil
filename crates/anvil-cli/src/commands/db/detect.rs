//! Which database a project uses and where its connection comes from, so `anvil db init` works
//! without flags: the project's own configuration first, a local database as the fallback.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use super::env_path::{env_files, parse_env};
use crate::commands::readiness::{DbKind, database_use};

/// Variables that say which engine a project talks to.
const ENGINE_VARS: &[&str] =
    &["DB_CONNECTION", "DB_TYPE", "DB_DIALECT", "DB_DRIVER", "DB_ENGINE", "TYPEORM_CONNECTION"];
/// Variables that hold part of a server connection.
const SERVER_VARS: &[&str] = &[
    "DATABASE_URL",
    "DB_HOST",
    "DATABASE_HOST",
    "POSTGRES_HOST",
    "PGHOST",
    "MYSQL_HOST",
    "POSTGRES_DB",
    "MYSQL_DATABASE",
];

/// What the project says about its database.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Detected {
    /// `sqlite`, `postgres` or `mysql`, when something names the engine.
    pub kind: Option<&'static str>,
    /// The env file that holds the server connection, repo-relative.
    pub env_file: Option<String>,
    /// Where `kind` was learned, for the message.
    pub origin: Option<String>,
}

/// Looks at the env files (engine names, `DATABASE_URL` schemes, connection variables), then at
/// the dependencies of the root and its direct subfolders.
pub(super) fn detect(root: &Path) -> Detected {
    let mut found = Detected::default();
    for (rel, _) in env_files(root) {
        let Ok(text) = fs::read_to_string(root.join(&rel)) else {
            continue;
        };
        let vars: BTreeMap<String, String> = parse_env(&text).into_iter().collect();
        let engine = ENGINE_VARS
            .iter()
            .find_map(|v| vars.get(*v).and_then(|value| engine_name(value)))
            .or_else(|| vars.get("DATABASE_URL").and_then(|url| engine_name(url)));
        let server = SERVER_VARS.iter().any(|v| vars.contains_key(*v));
        if found.kind.is_none() && engine.is_some() {
            found.kind = engine;
            found.origin = Some(rel.clone());
        }
        if found.env_file.is_none() && server && !rel.ends_with(".example") {
            found.env_file = Some(rel);
        }
    }
    if found.kind.is_none() {
        found.kind = dependency_kind(root).map(|(kind, dir)| {
            found.origin = Some(format!("{dir}dependencies"));
            kind
        });
    }
    found
}

/// `postgres`, `mysql` or `sqlite` for an engine name or a URL scheme.
fn engine_name(value: &str) -> Option<&'static str> {
    let lower = value.trim().to_ascii_lowercase();
    let head = lower.split("://").next().unwrap_or(&lower);
    let head = head.split(':').next().unwrap_or(head);
    match head {
        "pgsql" | "postgres" | "postgresql" | "pg" => Some("postgres"),
        "mysql" | "mariadb" | "mysql2" => Some("mysql"),
        "sqlite" | "sqlite3" | "better-sqlite3" | "file" => Some("sqlite"),
        _ => None,
    }
}

fn dependency_kind(root: &Path) -> Option<(&'static str, String)> {
    let mut dirs = vec![(root.to_path_buf(), String::new())];
    if let Ok(entries) = fs::read_dir(root) {
        let mut subdirs: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !n.starts_with('.') && n != "node_modules" && n != "target")
            .collect();
        subdirs.sort();
        dirs.extend(subdirs.into_iter().map(|n| (root.join(&n), format!("{n}/"))));
    }
    dirs.into_iter().find_map(|(dir, prefix)| {
        let kind = match database_use(&dir)? {
            DbKind::Postgres => "postgres",
            DbKind::Mysql => "mysql",
            DbKind::Sqlite => "sqlite",
            DbKind::Unknown => return None,
        };
        Some((kind, prefix))
    })
}

/// The local database used when the project configures none.
pub(super) fn local_defaults(kind: &str, database: &str) -> BTreeMap<String, String> {
    // The usual local development credentials (Toolbox refuses an empty password).
    let (port, user, password) =
        if kind == "mysql" { ("3306", "root", "root") } else { ("5432", "postgres", "postgres") };
    [
        ("DB_HOST", "localhost"),
        ("DB_PORT", port),
        ("DB_USER", user),
        ("DB_PASSWORD", password),
        ("DB_NAME", database),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(path, text).expect("write");
    }

    #[test]
    fn the_engine_comes_from_env_names_urls_or_dependencies() {
        let dir = tempfile::tempdir().expect("tmp");
        write(dir.path(), "api/.env", "DB_CONNECTION=pgsql\nDB_HOST=db\nDB_PASSWORD=x\n");
        let found = detect(dir.path());
        assert_eq!(found.kind, Some("postgres"));
        assert_eq!(found.env_file.as_deref(), Some("api/.env"));

        let url = tempfile::tempdir().expect("tmp");
        write(url.path(), ".env", "DATABASE_URL=mysql://u:p@h:3306/shop\n");
        assert_eq!(detect(url.path()).kind, Some("mysql"));

        let deps = tempfile::tempdir().expect("tmp");
        write(deps.path(), "svc/package.json", r#"{"dependencies":{"pg":"8"}}"#);
        let found = detect(deps.path());
        assert_eq!((found.kind, found.env_file), (Some("postgres"), None));
        assert_eq!(found.origin.as_deref(), Some("svc/dependencies"));
    }

    #[test]
    fn examples_name_the_engine_but_never_provide_the_connection() {
        let dir = tempfile::tempdir().expect("tmp");
        write(dir.path(), ".env.example", "DB_TYPE=mariadb\nDB_HOST=localhost\n");
        let found = detect(dir.path());
        assert_eq!((found.kind, found.env_file), (Some("mysql"), None));
        assert_eq!(local_defaults("mysql", "app").get("DB_PORT").map(String::as_str), Some("3306"));
    }
}
