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

/// Server frameworks: a project with one of these serves requests, so an embedded SQLite file
/// behind it is application data rather than a library's own storage.
const SERVERS: &[&str] = &[
    "\"express\"",
    "@nestjs/core",
    "\"fastify\"",
    "\"koa\"",
    "@hapi/hapi",
    "\"hono\"",
    "\"next\"",
    "\"nuxt\"",
    "@sveltejs/kit",
    "@remix-run/node",
    "@adonisjs/core",
    "fastapi",
    "flask",
    "django",
    "starlette",
    "aiohttp",
    "litestar",
    "laravel/framework",
    "symfony/http-kernel",
    "slim/slim",
    "axum",
    "actix-web",
    "rocket",
    "warp",
    "poem",
    "salvo",
    "gin-gonic/gin",
    "labstack/echo",
    "gofiber/fiber",
    "go-chi/chi",
    "gorilla/mux",
];

fn manifests_text(dir: &Path) -> String {
    MANIFESTS
        .iter()
        .filter_map(|m| fs::read_to_string(dir.join(m)).ok())
        .collect::<Vec<_>>()
        .join(
            "
",
        )
        .to_ascii_lowercase()
}

fn engine_of(text: &str) -> Option<DbKind> {
    let found: Vec<DbKind> =
        DRIVERS.iter().filter(|(dep, _)| text.contains(dep)).map(|(_, kind)| *kind).collect();
    found.iter().copied().find(|k| *k != DbKind::Unknown).or_else(|| found.first().copied())
}

/// The engine `dir`'s manifests depend on, whatever the project is: a concrete driver wins over
/// an ORM. `anvil db init` uses it, since the user asked for a database there.
pub fn driver_kind(dir: &Path) -> Option<DbKind> {
    engine_of(&manifests_text(dir))
}

/// The database `dir` serves data from, which its backend changes must be tested against. An
/// embedded SQLite driver only counts when the project is also a server: a library or CLI that
/// keeps its own state in SQLite (such as Anvil's memory) has no application database.
pub fn database_use(dir: &Path) -> Option<DbKind> {
    let text = manifests_text(dir);
    let kind = engine_of(&text)?;
    let server = SERVERS.iter().any(|framework| text.contains(framework));
    (kind != DbKind::Sqlite || server).then_some(kind)
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

    use super::{DbKind, database_use, driver_kind};

    fn with_manifest(name: &str, body: &str) -> TempDir {
        let dir = TempDir::new().expect("tmp");
        fs::write(dir.path().join(name), body).expect("write");
        dir
    }

    #[test]
    fn drivers_name_the_engine_and_orms_alone_leave_it_open() {
        let pg = with_manifest("package.json", r#"{"dependencies":{"typeorm":"1","pg":"8"}}"#);
        assert_eq!(database_use(pg.path()), Some(DbKind::Postgres));
        let lite = with_manifest(
            "package.json",
            r#"{"dependencies":{"@nestjs/core":"11","better-sqlite3":"12"}}"#,
        );
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

    #[test]
    fn embedded_sqlite_without_a_server_is_local_storage() {
        let memory = with_manifest("Cargo.toml", "[dependencies]\nrusqlite = \"0.32\"\n");
        assert_eq!(database_use(memory.path()), None);
        assert_eq!(driver_kind(memory.path()), Some(DbKind::Sqlite));
        let api =
            with_manifest("Cargo.toml", "[dependencies]\naxum = \"0.8\"\nrusqlite = \"0.32\"\n");
        assert_eq!(database_use(api.path()), Some(DbKind::Sqlite));
        let desktop = with_manifest("package.json", r#"{"dependencies":{"better-sqlite3":"12"}}"#);
        assert_eq!(database_use(desktop.path()), None);
    }
}
