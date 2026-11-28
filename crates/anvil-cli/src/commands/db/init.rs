//! `anvil db init`: writes the Toolbox tools file for the application database.

use std::fs;
use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::atomic_write;
use anvil_qa::api::db::SELECT_TOOL;
use anyhow::{Context, Result, bail};

use super::detect::{Detected, detect, local_defaults};
use super::env_path::find_sqlite_path;
use crate::ui;

pub(super) struct InitArgs<'a> {
    /// The engine; detected from the project when `None`.
    pub kind: Option<&'a str>,
    pub database: Option<&'a str>,
    pub name: &'a str,
    pub toolbox: Option<&'a str>,
    pub force: bool,
}

pub(super) fn run(root: &Path, args: &InitArgs<'_>) -> Result<()> {
    let mut config = AnvilConfig::load_or_default(root)?;
    let target = config.db.config_path(root);
    if target.exists() && !args.force {
        bail!("{} already exists; pass --force to overwrite it", config.db.config);
    }
    let detected = detect(root);
    let kind = choose_kind(args.kind, &detected)?;
    let database = resolve_database(root, kind, args.database)?;
    let yaml = tools_yaml(kind, args.name, database.as_deref())?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    atomic_write(&target, yaml.as_bytes())?;
    if let Some(toolbox) = args.toolbox {
        config.db.toolbox = toolbox.to_owned();
    }
    set_connection(root, &mut config, kind, &detected, args.database);
    config.save(root)?;
    println!("wrote {} ({kind} source `{}`)", config.db.config, args.name);
    print_next_steps(kind, &config.db.toolbox);
    Ok(())
}

fn choose_kind(given: Option<&str>, detected: &Detected) -> Result<&'static str> {
    let kind = match given {
        Some("sqlite") => "sqlite",
        Some("postgres") => "postgres",
        Some("mysql") => "mysql",
        Some(other) => {
            bail!("unsupported database kind `{other}` (use sqlite, postgres, or mysql)")
        }
        None => {
            let Some(kind) = detected.kind else {
                bail!(
                    "could not tell which database the project uses (no DB_CONNECTION/DATABASE_URL in its .env files, no known driver in its dependencies); pass --kind sqlite|postgres|mysql"
                );
            };
            let origin = detected.origin.as_deref().unwrap_or("the project");
            println!("detected {kind} from {origin}");
            kind
        }
    };
    Ok(kind)
}

/// Server databases read `${DB_*}` from the project's env file when it has a connection, else
/// from local defaults (localhost and the engine's default port and user).
fn set_connection(
    root: &Path,
    config: &mut AnvilConfig,
    kind: &str,
    detected: &Detected,
    database: Option<&str>,
) {
    config.db.env_file = None;
    config.db.defaults.clear();
    if kind == "sqlite" {
        return;
    }
    let name = database.map(str::to_owned).unwrap_or_else(|| {
        root.file_name()
            .map_or_else(|| "app".to_owned(), |n| n.to_string_lossy().replace([' ', '-'], "_"))
    });
    // The local defaults also fill whatever the env file leaves out: Toolbox refuses a file
    // with an unset placeholder.
    config.db.defaults = local_defaults(kind, &name);
    if let Some(env_file) = &detected.env_file {
        println!(
            "connection: the DB_* / DATABASE_URL variables of {env_file} (read when Toolbox runs; nothing is copied), local defaults for any it lacks"
        );
        config.db.env_file = Some(env_file.clone());
        return;
    }
    let port = config.db.defaults.get("DB_PORT").cloned().unwrap_or_default();
    println!(
        "connection: the project configures no {kind} database, so a local one is used (localhost:{port}, database `{name}`); set DB_HOST/DB_PORT/DB_USER/DB_PASSWORD/DB_NAME or DATABASE_URL to use another"
    );
}

/// The `database` value: given, found in `.env*` files (SQLite), or left to an env placeholder.
fn resolve_database(root: &Path, kind: &str, given: Option<&str>) -> Result<Option<String>> {
    if let Some(database) = given {
        return Ok(Some(database.replace('\\', "/")));
    }
    if kind != "sqlite" {
        return Ok(None);
    }
    let Some(found) = find_sqlite_path(root) else {
        bail!(
            "no SQLite path found in the .env files; pass --database <file relative to the repository root>"
        );
    };
    println!("using {} from {}: {}", found.variable, found.file, found.path);
    if !root.join(&found.path).is_file() {
        println!(
            "  {} {} does not exist yet; scenarios need it once the API has created it",
            ui::yellow("note:"),
            found.path
        );
    }
    Ok(Some(found.path))
}

/// Generates the YAML: one source plus the managed read-only tool. Server credentials are
/// `${VAR}` placeholders that Toolbox fills from the environment, never literal values.
fn tools_yaml(kind: &str, name: &str, database: Option<&str>) -> Result<String> {
    let source_fields = match kind {
        "sqlite" => {
            let Some(path) = database else { bail!("a SQLite source needs --database") };
            format!("type: sqlite\ndatabase: {}\n", yaml_scalar(path))
        }
        "postgres" | "mysql" => format!(
            "type: {kind}\nhost: ${{DB_HOST}}\nport: ${{DB_PORT}}\ndatabase: {}\nuser: ${{DB_USER}}\npassword: ${{DB_PASSWORD}}\n",
            database.map_or_else(|| "${DB_NAME}".to_owned(), yaml_scalar)
        ),
        other => bail!("unsupported database kind `{other}` (use sqlite, postgres, or mysql)"),
    };
    // Comments must not contain the placeholder syntax: Toolbox substitutes it everywhere.
    Ok(format!(
        "# Google MCP Toolbox for Databases tools for this project (managed by `anvil db init`).\n\
         # Keep credentials in environment variables, written as a dollar sign and the name in braces.\n\
         kind: source\nname: {name}\n{source_fields}---\n\
         kind: tool\nname: {SELECT_TOOL}\ntype: {kind}-execute-sql\nsource: {name}\n\
         description: Run one read-only SELECT against the application database and return its rows.\n"
    ))
}

/// Plain when safe, else a double-quoted (JSON-style) YAML string.
fn yaml_scalar(text: &str) -> String {
    let plain = !text.is_empty()
        && text.chars().all(|c| c.is_ascii_alphanumeric() || "_-./$:{}".contains(c))
        && !text.starts_with(['-', ':']);
    if plain {
        text.to_owned()
    } else {
        serde_json::to_string(text).unwrap_or_else(|_| format!("\"{text}\""))
    }
}

fn print_next_steps(kind: &str, toolbox: &str) {
    println!("next steps:");
    if kind != "sqlite" {
        println!(
            "  - use a read-only database user: the SELECT-only rule is a guard, not a permission system"
        );
    }
    println!(
        "  - install Toolbox (https://github.com/googleapis/mcp-toolbox/releases) and set `db.toolbox` if it is not `{toolbox}` on PATH"
    );
    println!("  - anvil db check                  verify the binary, the file, and SELECT 1");
    println!("  - anvil db query \"SELECT 1\"       look at the data");
    println!("  - anvil db mcp on                 let agents query it (MCP server `anvil-db`)");
    println!("  - add scenarios in .harness/qa/api/ and run `anvil qa api`");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_gets_the_path_and_the_managed_tool() {
        let yaml =
            tools_yaml("sqlite", "app-db", Some("orders-api/data/orders.sqlite")).expect("yaml");
        assert!(yaml.contains("kind: source\nname: app-db\ntype: sqlite\ndatabase: orders-api/data/orders.sqlite\n"), "{yaml}");
        assert!(
            yaml.contains(
                "kind: tool\nname: anvil_select\ntype: sqlite-execute-sql\nsource: app-db\n"
            ),
            "{yaml}"
        );
        assert!(tools_yaml("sqlite", "app-db", None).is_err());
    }

    #[test]
    fn server_databases_use_placeholders_and_never_literal_credentials() {
        for kind in ["postgres", "mysql"] {
            let yaml = tools_yaml(kind, "app-db", None).expect("yaml");
            for var in ["DB_HOST", "DB_PORT", "DB_USER", "DB_PASSWORD", "DB_NAME"] {
                assert!(yaml.contains(&format!("${{{var}}}")), "{kind}: {var} missing in {yaml}");
            }
            assert!(yaml.contains(&format!("type: {kind}-execute-sql")), "{yaml}");
        }
        let named = tools_yaml("postgres", "app-db", Some("shop")).expect("yaml");
        assert!(named.contains("database: shop\n") && !named.contains("DB_NAME"), "{named}");
        let comments: Vec<&str> = named.lines().filter(|l| l.starts_with('#')).collect();
        assert!(comments.iter().all(|c| !c.contains("${")), "{comments:?}");
        assert!(tools_yaml("oracle", "x", None).is_err());
    }

    #[test]
    fn awkward_paths_are_quoted() {
        assert_eq!(yaml_scalar("data/my app.db"), "\"data/my app.db\"");
        assert_eq!(yaml_scalar("data/app.db"), "data/app.db");
    }
}
