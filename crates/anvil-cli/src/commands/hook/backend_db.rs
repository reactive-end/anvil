//! `Stop` gate: a turn that changed backend code of a project that uses a database is only done
//! once the API scenarios passed against that database after the change. Missing setup (no
//! Toolbox tools file, no scenario) blocks with the command that fixes it.

use std::path::Path;
use std::time::SystemTime;

use anvil_adapters::contract::HookOutcome;
use anvil_core::config::AnvilConfig;
use anvil_core::locale::Lang;
use anvil_qa::api::run::last_pass;
use anvil_qa::api::scenarios_dir;
use anvil_qa::api::spec::load_scenarios;

use crate::commands::prereqs::projects_for;
use crate::commands::readiness::database_use;

/// Extensions of server-side source files.
const SOURCE_EXTENSIONS: &[&str] =
    &["ts", "js", "mjs", "cjs", "py", "php", "rs", "go", "java", "kt", "cs", "rb"];

/// Blocks `Stop` while backend changes are not verified against the database.
pub fn stop_block(
    root: &Path,
    config: &AnvilConfig,
    changed: &[String],
    lang: Lang,
) -> Option<HookOutcome> {
    let files = backend_changes(root, config, changed);
    let first = files.first()?;
    let reason = if !config.db.config_path(root).is_file() {
        Some(texts(lang).no_database)
    } else if config.qa.api.url.is_none() || !has_scenarios(root, config) {
        Some(texts(lang).no_scenario)
    } else if !verified_after(root, &files) {
        Some(texts(lang).not_run)
    } else {
        None
    }?;
    let more = files.len().saturating_sub(1);
    let what = if more == 0 { first.clone() } else { format!("{first} (+{more})") };
    Some(HookOutcome::block_stop(format!("{} `{what}`. {reason}", texts(lang).lead)))
}

/// Changed source files (tests and docs aside) of the projects whose dependencies use a database.
fn backend_changes(root: &Path, config: &AnvilConfig, changed: &[String]) -> Vec<String> {
    let Ok(projects) = projects_for(root, config) else {
        return Vec::new();
    };
    let prefixes: Vec<String> = projects
        .iter()
        .filter(|p| database_use(&root.join(&p.rel_path)).is_some())
        .map(|p| if p.rel_path == "." { String::new() } else { format!("{}/", p.rel_path) })
        .collect();
    changed
        .iter()
        .filter(|file| is_backend_source(file))
        .filter(|file| prefixes.iter().any(|prefix| file.starts_with(prefix.as_str())))
        .cloned()
        .collect()
}

fn is_backend_source(file: &str) -> bool {
    let unified = file.replace('\\', "/");
    let lower = unified.to_ascii_lowercase();
    let is_test = [".spec.", ".test.", "/test/", "/tests/", "__tests__", "/e2e/"]
        .iter()
        .any(|marker| lower.contains(marker))
        || lower.starts_with("test/")
        || lower.starts_with("tests/");
    let extension = lower.rsplit('.').next().unwrap_or_default();
    !lower.starts_with(".harness/") && !is_test && SOURCE_EXTENSIONS.contains(&extension)
}

fn has_scenarios(root: &Path, config: &AnvilConfig) -> bool {
    load_scenarios(&scenarios_dir(root, &config.qa.api)).is_ok_and(|entries| !entries.is_empty())
}

/// The last passing run is newer than every changed file (a deleted file counts as just now).
fn verified_after(root: &Path, files: &[String]) -> bool {
    let Some(passed) = last_pass(root) else {
        return false;
    };
    files.iter().all(|file| {
        let modified = std::fs::metadata(root.join(file))
            .and_then(|m| m.modified())
            .unwrap_or_else(|_| SystemTime::now());
        passed >= modified
    })
}

struct Texts {
    lead: &'static str,
    no_database: &'static str,
    no_scenario: &'static str,
    not_run: &'static str,
}

fn texts(lang: Lang) -> Texts {
    match lang {
        Lang::Es => Texts {
            lead: "Cambio de backend sin verificar contra la base de datos:",
            no_database: "Configura la BD con `anvil db init` (usa la del proyecto y, si no hay, una local) y `anvil db mcp on`, crea un escenario en .harness/qa/api que llame al endpoint cambiado y compruebe las filas, y ejecuta `anvil qa api --start`.",
            no_scenario: "Crea un escenario en .harness/qa/api (con `qa.api.url` configurado) que llame al endpoint cambiado y compruebe en la BD lo que debe guardarse, y ejecuta `anvil qa api --start`.",
            not_run: "Ejecuta `anvil qa api --start` (o `anvil check`) para comprobar en la BD que el cambio hace lo esperado; si un escenario falla, corrige el código, no el escenario.",
        },
        Lang::En => Texts {
            lead: "Backend change not verified against the database:",
            no_database: "Set up the database with `anvil db init` (the project's own, else a local one) and `anvil db mcp on`, add a scenario in .harness/qa/api that calls the changed endpoint and checks the rows, then run `anvil qa api --start`.",
            no_scenario: "Add a scenario in .harness/qa/api (with `qa.api.url` set) that calls the changed endpoint and checks in the database what must be stored, then run `anvil qa api --start`.",
            not_run: "Run `anvil qa api --start` (or `anvil check`) to confirm in the database that the change does what it should; when a scenario fails, fix the code, not the scenario.",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::is_backend_source;

    #[test]
    fn only_server_source_counts_as_a_backend_change() {
        for yes in ["api/src/orders/pay.use-case.ts", "app/models.py", "src/main.rs"] {
            assert!(is_backend_source(yes), "{yes}");
        }
        for no in [
            "api/src/orders/pay.spec.ts",
            "api/test/app.e2e.ts",
            "README.md",
            ".harness/qa/api/a.json",
            "api/package.json",
        ] {
            assert!(!is_backend_source(no), "{no}");
        }
    }
}
