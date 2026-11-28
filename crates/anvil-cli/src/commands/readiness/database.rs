//! Projects that talk to a database need Google MCP Toolbox: its binary, the tools file, and the
//! `anvil-db` MCP server, so backend changes can be checked against the data they write.

use std::fs;
use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_prereqs::{Item, Kind, Project, Status};
use anvil_qa::api::db::{DOWNLOAD_HINT, ToolboxClient};

/// Database engine a project's dependencies point at, as `anvil db init --kind` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbKind {
    Postgres,
    Mysql,
    Sqlite,
    /// An ORM or framework that can sit on any engine.
    Unknown,
}

impl DbKind {
    pub fn init_flag(self) -> &'static str {
        match self {
            Self::Postgres => " --kind postgres",
            Self::Mysql => " --kind mysql",
            Self::Sqlite => " --kind sqlite",
            Self::Unknown => "",
        }
    }
}

/// Dependency names (as they appear in manifests) and the engine they imply.
const DRIVERS: &[(&str, DbKind)] = &[
    ("\"pg\"", DbKind::Postgres),
    ("\"postgres\"", DbKind::Postgres),
    ("psycopg", DbKind::Postgres),
    ("asyncpg", DbKind::Postgres),
    ("tokio-postgres", DbKind::Postgres),
    ("jackc/pgx", DbKind::Postgres),
    ("\"mysql2\"", DbKind::Mysql),
    ("\"mariadb\"", DbKind::Mysql),
    ("pymysql", DbKind::Mysql),
    ("mysqlclient", DbKind::Mysql),
    ("\"better-sqlite3\"", DbKind::Sqlite),
    ("\"sqlite3\"", DbKind::Sqlite),
    ("rusqlite", DbKind::Sqlite),
    ("\"typeorm\"", DbKind::Unknown),
    ("\"prisma\"", DbKind::Unknown),
    ("@prisma/client", DbKind::Unknown),
    ("\"sequelize\"", DbKind::Unknown),
    ("\"knex\"", DbKind::Unknown),
    ("drizzle-orm", DbKind::Unknown),
    ("@mikro-orm/core", DbKind::Unknown),
    ("sqlalchemy", DbKind::Unknown),
    ("django", DbKind::Unknown),
    ("doctrine/orm", DbKind::Unknown),
    ("laravel/framework", DbKind::Unknown),
    ("sqlx", DbKind::Unknown),
    ("diesel", DbKind::Unknown),
    ("sea-orm", DbKind::Unknown),
    ("gorm.io", DbKind::Unknown),
];

const MANIFESTS: &[&str] = &[
    "package.json",
    "composer.json",
    "requirements.txt",
    "pyproject.toml",
    "Cargo.toml",
    "go.mod",
];

/// The engine `dir`'s manifests depend on: a concrete driver wins over an ORM.
pub fn database_use(dir: &Path) -> Option<DbKind> {
    let text: String = MANIFESTS
        .iter()
        .filter_map(|m| fs::read_to_string(dir.join(m)).ok())
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    let found: Vec<DbKind> =
        DRIVERS.iter().filter(|(dep, _)| text.contains(dep)).map(|(_, kind)| *kind).collect();
    found.iter().copied().find(|k| *k != DbKind::Unknown).or_else(|| found.first().copied())
}

/// Toolbox items for the first project that uses a database (the setup is shared by the repo).
pub fn items(root: &Path, config: &AnvilConfig, projects: &[Project]) -> Vec<Item> {
    let Some((project, kind)) =
        projects.iter().find_map(|p| database_use(&root.join(&p.rel_path)).map(|kind| (p, kind)))
    else {
        return Vec::new();
    };
    let subrepo = project.name.as_deref();
    let source = "database dependencies";
    let client = ToolboxClient::new(root, &config.db);
    let binary = match client.version() {
        Ok(version) => Item::new(subrepo, Kind::Tool, "toolbox", Status::Ok).found(Some(version)),
        Err(_) => Item::new(subrepo, Kind::Tool, "toolbox", Status::Missing)
            .message("MCP Toolbox for Databases is needed to check backend changes against the database")
            .hint(&format!("download it from {DOWNLOAD_HINT} and set `db.toolbox` (`anvil db init --toolbox <path>`)")),
    };
    let tools_file = config.db.config_path(root);
    let file = if tools_file.is_file() {
        Item::new(subrepo, Kind::Path, &config.db.config, Status::Ok)
    } else {
        Item::new(subrepo, Kind::Path, &config.db.config, Status::Missing)
            .message("no Toolbox tools file for the project's database")
            .hint(&format!(
                "`anvil db init{}` (uses the configured database, else a local one)",
                kind.init_flag()
            ))
    };
    let mcp = if config.db.mcp {
        Item::new(subrepo, Kind::Custom, "anvil-db MCP server", Status::Ok)
    } else {
        Item::new(subrepo, Kind::Custom, "anvil-db MCP server", Status::Missing)
            .message("agents cannot query the database without the anvil-db MCP server")
            .hint("`anvil db mcp on`")
    };
    [binary, file, mcp].into_iter().map(|item| item.source(source)).collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{DbKind, database_use};

    fn with_manifest(name: &str, body: &str) -> TempDir {
        let dir = TempDir::new().expect("tmp");
        fs::write(dir.path().join(name), body).expect("write");
        dir
    }

    #[test]
    fn drivers_name_the_engine_and_orms_alone_leave_it_open() {
        let pg = with_manifest("package.json", r#"{"dependencies":{"typeorm":"1","pg":"8"}}"#);
        assert_eq!(database_use(pg.path()), Some(DbKind::Postgres));
        let lite = with_manifest("package.json", r#"{"dependencies":{"better-sqlite3":"12"}}"#);
        assert_eq!(database_use(lite.path()), Some(DbKind::Sqlite));
        let orm = with_manifest("composer.json", r#"{"require":{"laravel/framework":"^11"}}"#);
        assert_eq!(database_use(orm.path()), Some(DbKind::Unknown));
        let py = with_manifest("requirements.txt", "fastapi\npsycopg[binary]\n");
        assert_eq!(database_use(py.path()), Some(DbKind::Postgres));
    }

    #[test]
    fn projects_without_a_database_need_nothing() {
        let web =
            with_manifest("package.json", r#"{"devDependencies":{"vite":"8","pgadmin-ui":"1"}}"#);
        assert_eq!(database_use(web.path()), None);
        let cli = with_manifest("Cargo.toml", "[dependencies]\nclap = \"4\"\n");
        assert_eq!(database_use(cli.path()), None);
    }
}
