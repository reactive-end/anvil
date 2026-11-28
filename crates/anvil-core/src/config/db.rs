//! `db` section of `.harness/config.json`: the Google MCP Toolbox for Databases setup that scenarios
//! and agents use to look at the application database.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Toolbox binary, its tools file, and whether agents get it as an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbConfig {
    /// A bare name looked up on `PATH`, or a path relative to the repository root.
    #[serde(default = "default_toolbox", skip_serializing_if = "is_default_toolbox")]
    pub toolbox: String,
    /// Toolbox tools file (relative to the repository root). Toolbox always runs from the root,
    /// so relative SQLite paths inside it are written relative to the root too.
    #[serde(default = "default_config", skip_serializing_if = "is_default_config")]
    pub config: String,
    /// Register the tools file as the `anvil-db` MCP server in every adapter.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mcp: bool,
    /// Repo-relative `.env` file whose variables Anvil passes to Toolbox, so the `${DB_*}`
    /// placeholders of the tools file resolve without copying credentials anywhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_file: Option<String>,
    /// Values used when neither the environment nor `env_file` sets a placeholder: the local
    /// database `anvil db init` falls back to when the project configures none.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub defaults: BTreeMap<String, String>,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            toolbox: default_toolbox(),
            config: default_config(),
            mcp: false,
            env_file: None,
            defaults: BTreeMap::new(),
        }
    }
}

/// Names projects use for the parts of a connection, mapped to the `DB_*` placeholders.
const ALIASES: &[(&str, &[&str])] = &[
    ("DB_HOST", &["DB_HOST", "DATABASE_HOST", "POSTGRES_HOST", "PGHOST", "MYSQL_HOST"]),
    ("DB_PORT", &["DB_PORT", "DATABASE_PORT", "POSTGRES_PORT", "PGPORT", "MYSQL_PORT"]),
    (
        "DB_USER",
        &["DB_USER", "DB_USERNAME", "DATABASE_USER", "POSTGRES_USER", "PGUSER", "MYSQL_USER"],
    ),
    (
        "DB_PASSWORD",
        &[
            "DB_PASSWORD",
            "DB_PASS",
            "DATABASE_PASSWORD",
            "POSTGRES_PASSWORD",
            "PGPASSWORD",
            "MYSQL_PASSWORD",
        ],
    ),
    (
        "DB_NAME",
        &["DB_NAME", "DB_DATABASE", "DATABASE_NAME", "POSTGRES_DB", "PGDATABASE", "MYSQL_DATABASE"],
    ),
];

/// The `DB_*` values Toolbox needs, from (lowest to highest priority) the local defaults, the
/// ambient environment (`DATABASE_URL`, `PGUSER`, `MYSQL_PASSWORD`, ...), the project's
/// `env_file` (aliases and a `DATABASE_URL` included), and `DB_*` set in the environment.
pub fn connection_env(root: &Path, db: &DbConfig) -> Vec<(String, String)> {
    connection_env_from(root, db, &std::env::vars().collect())
}

/// [`connection_env`] with the process environment given. Generic variables such as `PGUSER`
/// are often set machine-wide (CI runners do), so they never override the project's own file;
/// only the exact `DB_*` names are an explicit override.
fn connection_env_from(
    root: &Path,
    db: &DbConfig,
    process: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let file = db.env_file.as_deref().map(|f| read_env_file(&root.join(f))).unwrap_or_default();
    let mut out = db.defaults.clone();
    for layer in [process, &file] {
        if let Some(url) = layer.get("DATABASE_URL").and_then(|u| parse_url(u)) {
            out.extend(url);
        }
        for (placeholder, names) in ALIASES {
            if let Some(value) = names.iter().find_map(|n| layer.get(*n)) {
                out.insert((*placeholder).to_owned(), value.clone());
            }
        }
    }
    for (placeholder, _) in ALIASES {
        if let Some(value) = process.get(*placeholder) {
            out.insert((*placeholder).to_owned(), value.clone());
        }
    }
    out.into_iter().collect()
}

/// `KEY=value` lines of a `.env` file (quotes and `export` stripped); unreadable means empty.
pub fn read_env_file(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_prefix("export ").unwrap_or(line);
            if line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            Some((key.trim().to_owned(), value.to_owned()))
        })
        .collect()
}

/// `scheme://user:password@host:port/name?params` split into `DB_*` values.
fn parse_url(url: &str) -> Option<BTreeMap<String, String>> {
    let (_, rest) = url.split_once("://")?;
    let (auth_host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (auth, host_port) = auth_host.rsplit_once('@').unwrap_or(("", auth_host));
    let (user, password) = auth.split_once(':').unwrap_or((auth, ""));
    let (host, port) = host_port.rsplit_once(':').unwrap_or((host_port, ""));
    let name = path.split('?').next().unwrap_or_default();
    let pairs = [
        ("DB_HOST", host),
        ("DB_PORT", port),
        ("DB_USER", user),
        ("DB_PASSWORD", password),
        ("DB_NAME", name),
    ];
    Some(
        pairs
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
    )
}

impl DbConfig {
    /// `true` when nothing was configured, so the section is left out of the saved file.
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    /// Absolute location of the tools file.
    pub fn config_path(&self, root: &Path) -> PathBuf {
        root.join(&self.config)
    }

    /// The program to run: the bare name for `PATH` lookup, or a path resolved against `root`.
    pub fn toolbox_program(&self, root: &Path) -> PathBuf {
        let has_separator = self.toolbox.contains(['/', '\\']);
        if has_separator { root.join(&self.toolbox) } else { PathBuf::from(&self.toolbox) }
    }
}

fn default_toolbox() -> String {
    "toolbox".to_owned()
}

fn default_config() -> String {
    ".harness/db/tools.yaml".to_owned()
}

fn is_default_toolbox(value: &String) -> bool {
    value == "toolbox"
}

fn is_default_config(value: &String) -> bool {
    value == ".harness/db/tools.yaml"
}

fn is_false(flag: &bool) -> bool {
    !flag
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_left_out_and_restored() {
        let json = serde_json::to_value(DbConfig::default()).expect("serialize");
        assert_eq!(json, serde_json::json!({}));
        let parsed: DbConfig = serde_json::from_str("{}").expect("parse");
        assert!(parsed.is_default());
        let set: DbConfig = serde_json::from_str(r#"{"mcp":true}"#).expect("parse");
        assert!(set.mcp && set.toolbox == "toolbox");
    }

    #[test]
    fn the_connection_comes_from_defaults_then_the_env_file_and_its_aliases() {
        let dir = tempfile::TempDir::new().expect("tmp");
        std::fs::write(
            dir.path().join(".env"),
            "# app\nDB_USERNAME=app\nexport DB_DATABASE=\"shop\"\nDATABASE_URL=postgres://u:p%40ss@db.local:6543/main?ssl=1\n",
        )
        .expect("env");
        let mut defaults = BTreeMap::new();
        defaults.insert("DB_HOST".to_owned(), "localhost".to_owned());
        defaults.insert("DB_PORT".to_owned(), "5432".to_owned());
        let db = DbConfig { env_file: Some(".env".to_owned()), defaults, ..DbConfig::default() };
        let ambient: BTreeMap<String, String> = [("PGUSER", "runner"), ("PGPASSWORD", "root")]
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .into();
        let env: BTreeMap<String, String> =
            connection_env_from(dir.path(), &db, &ambient).into_iter().collect();
        assert_eq!(env.get("DB_HOST").map(String::as_str), Some("db.local"));
        assert_eq!(env.get("DB_PORT").map(String::as_str), Some("6543"));
        assert_eq!(env.get("DB_USER").map(String::as_str), Some("app"), "alias beats the URL");
        assert_eq!(env.get("DB_NAME").map(String::as_str), Some("shop"));
        assert_eq!(env.get("DB_PASSWORD").map(String::as_str), Some("p%40ss"));
        let local = DbConfig { defaults: db.defaults.clone(), ..DbConfig::default() };
        let env: BTreeMap<String, String> =
            connection_env_from(dir.path(), &local, &ambient).into_iter().collect();
        assert_eq!(env.get("DB_PORT").map(String::as_str), Some("5432"), "local fallback");
        assert_eq!(env.get("DB_PASSWORD").map(String::as_str), Some("root"), "ambient fills gaps");
        let explicit: BTreeMap<String, String> =
            [("DB_USER".to_owned(), "override".to_owned())].into();
        let env: BTreeMap<String, String> =
            connection_env_from(dir.path(), &db, &explicit).into_iter().collect();
        assert_eq!(env.get("DB_USER").map(String::as_str), Some("override"), "DB_* wins");
    }

    #[test]
    fn bare_names_use_path_and_separators_mean_repo_relative() {
        let root = Path::new("/repo");
        assert_eq!(DbConfig::default().toolbox_program(root), PathBuf::from("toolbox"));
        let local = DbConfig { toolbox: ".tools/toolbox.exe".to_owned(), ..DbConfig::default() };
        assert_eq!(local.toolbox_program(root), root.join(".tools/toolbox.exe"));
    }
}
