//! Scenario files (`.harness/qa/api/<name>.json`): parsing and validation.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::Value;

use crate::listing::{file_id, json_files, same_name};

const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// One scenario: variables plus the steps that run in order.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Starting variables; string values may use `{{$run}}`.
    #[serde(default)]
    pub vars: BTreeMap<String, Value>,
    pub steps: Vec<Step>,
}

/// A step is exactly one of `http` or `db`, with optional expectations and captures.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub http: Option<HttpStep>,
    #[serde(default)]
    pub db: Option<DbStep>,
    #[serde(default)]
    pub expect: Option<Expect>,
    /// Variable name to JSON pointer into the response body (HTTP) or the rows array (DB).
    #[serde(default)]
    pub capture: BTreeMap<String, String>,
}

/// A request to the API under test.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpStep {
    pub method: String,
    /// Relative to `qa.api.url`, or an absolute URL.
    pub path: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub json: Option<Value>,
    #[serde(default)]
    pub body: Option<String>,
}

/// A database lookup: a tool of the Toolbox config, or a read-only `sql` through `anvil_select`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DbStep {
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub sql: Option<String>,
}

/// What a step must have produced. HTTP steps use `status`, `json`, `body_contains`, and `headers`;
/// DB steps use `rows`, `row`, and `contains`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    #[serde(default)]
    pub status: Option<u16>,
    /// JSON pointer to expected value, checked against the response body.
    #[serde(default)]
    pub json: BTreeMap<String, Value>,
    #[serde(default)]
    pub body_contains: Option<String>,
    /// Header name to a substring of its value.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub rows: Option<usize>,
    /// Subset of columns the first row must have.
    #[serde(default)]
    pub row: Option<Value>,
    /// Subset of columns that at least one row must have.
    #[serde(default)]
    pub contains: Option<Value>,
}

impl Expect {
    fn uses_http_fields(&self) -> bool {
        self.status.is_some()
            || !self.json.is_empty()
            || self.body_contains.is_some()
            || !self.headers.is_empty()
    }

    fn uses_db_fields(&self) -> bool {
        self.rows.is_some() || self.row.is_some() || self.contains.is_some()
    }
}

impl Step {
    /// The label used in messages: the `name`, else a short description of the call.
    pub fn label(&self) -> String {
        if let Some(name) = self.name.as_deref().filter(|n| !n.trim().is_empty()) {
            return name.to_owned();
        }
        match (&self.http, &self.db) {
            (Some(http), _) => format!("{} {}", http.method.to_uppercase(), http.path),
            (_, Some(db)) => db.label(),
            _ => "step".to_owned(),
        }
    }

    /// `"http"` or `"db"`.
    pub fn kind(&self) -> &'static str {
        if self.http.is_some() { "http" } else { "db" }
    }

    fn validate(&self) -> Result<()> {
        match (&self.http, &self.db) {
            (Some(http), None) => self.validate_http(http),
            (None, Some(db)) => self.validate_db(db),
            _ => bail!("must have exactly one of `http` or `db`"),
        }
    }

    fn validate_http(&self, http: &HttpStep) -> Result<()> {
        if !METHODS.contains(&http.method.to_uppercase().as_str()) {
            bail!("unsupported HTTP method `{}` (use one of {})", http.method, METHODS.join(", "));
        }
        if http.path.trim().is_empty() {
            bail!("`http.path` must not be empty");
        }
        if http.json.is_some() && http.body.is_some() {
            bail!("`http.json` and `http.body` are mutually exclusive");
        }
        if self.expect.as_ref().is_some_and(Expect::uses_db_fields) {
            bail!("`rows`, `row`, and `contains` only apply to `db` steps");
        }
        check_pointers(self.expect.iter().flat_map(|e| e.json.keys()).chain(self.capture.values()))
    }

    fn validate_db(&self, db: &DbStep) -> Result<()> {
        match (&db.tool, &db.sql) {
            (Some(_), Some(_)) => bail!("`db.tool` and `db.sql` are mutually exclusive"),
            (None, None) => bail!("`db` needs a `tool` or a `sql`"),
            (None, Some(_)) if db.params.is_some() => {
                bail!("`db.params` only applies to `db.tool`; put values into the `sql` text")
            }
            _ => {}
        }
        if db.params.as_ref().is_some_and(|p| !p.is_object()) {
            bail!("`db.params` must be an object");
        }
        if self.expect.as_ref().is_some_and(Expect::uses_http_fields) {
            bail!("`status`, `json`, `body_contains`, and `headers` only apply to `http` steps");
        }
        let mut objects = self.expect.iter().flat_map(|e| e.row.iter().chain(e.contains.iter()));
        if objects.any(|v| !v.is_object()) {
            bail!("`row` and `contains` must be objects of column values");
        }
        check_pointers(self.capture.values())
    }
}

impl DbStep {
    fn label(&self) -> String {
        match (&self.tool, &self.sql) {
            (Some(tool), _) => format!("db {tool}"),
            (_, Some(sql)) => {
                format!("db {}", sql.split_whitespace().collect::<Vec<_>>().join(" "))
            }
            _ => "db".to_owned(),
        }
    }
}

fn check_pointers<'a>(pointers: impl Iterator<Item = &'a String>) -> Result<()> {
    for pointer in pointers {
        if !pointer.is_empty() && !pointer.starts_with('/') {
            bail!("`{pointer}` is not a JSON pointer (it must be empty or start with `/`)");
        }
    }
    Ok(())
}

impl Scenario {
    /// Parses and validates a scenario file.
    pub fn parse(raw: &str) -> Result<Self> {
        let text = raw.strip_prefix('\u{feff}').unwrap_or(raw);
        let scenario: Self = serde_json::from_str(text)?;
        scenario.validate()?;
        Ok(scenario)
    }

    fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("`name` must not be empty");
        }
        if self.steps.is_empty() {
            bail!("`steps` must not be empty");
        }
        for (index, step) in self.steps.iter().enumerate() {
            step.validate().map_err(|err| anyhow::anyhow!("step {}: {err}", index + 1))?;
        }
        Ok(())
    }

    /// `true` when some step reads the database.
    pub fn uses_db(&self) -> bool {
        self.steps.iter().any(|s| s.db.is_some())
    }
}

/// A scenario file found on disk, valid or not.
#[derive(Debug)]
pub struct ScenarioEntry {
    /// File stem; names the scenario on the command line.
    pub id: String,
    pub path: PathBuf,
    pub scenario: std::result::Result<Scenario, String>,
}

/// Reads every `*.json` file of `dir` (sorted by name); a missing directory has no scenarios.
pub fn load_scenarios(dir: &Path) -> Result<Vec<ScenarioEntry>> {
    Ok(json_files(dir)?.into_iter().map(read_entry).collect())
}

fn read_entry(path: PathBuf) -> ScenarioEntry {
    let id = file_id(&path);
    let scenario = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))
        .and_then(|raw| Scenario::parse(&raw).map_err(|err| format!("{err:#}")));
    ScenarioEntry { id, path, scenario }
}

/// Picks the scenario whose file stem or `name` equals `wanted` (ignoring case, spaces, `-`/`_`).
pub fn find_scenario<'a>(entries: &'a [ScenarioEntry], wanted: &str) -> Option<&'a ScenarioEntry> {
    entries.iter().find(|entry| {
        same_name(&entry.id, wanted)
            || entry.scenario.as_ref().is_ok_and(|s| same_name(&s.name, wanted))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{
        "name": "create order",
        "vars": { "email": "qa-{{$run}}@example.com" },
        "steps": [
            { "http": { "method": "post", "path": "/customers", "json": { "email": "{{email}}" } },
              "expect": { "status": 201 }, "capture": { "customerId": "/id" } },
            { "name": "row exists", "db": { "sql": "SELECT id FROM customers" },
              "expect": { "rows": 1, "row": { "id": "{{customerId}}" } } }
        ]
    }"#;

    fn parse_err(raw: &str) -> String {
        format!("{:#}", Scenario::parse(raw).expect_err("must be rejected"))
    }

    #[test]
    fn a_valid_scenario_parses_and_labels_its_steps() {
        let scenario = Scenario::parse(GOOD).expect("parse");
        assert_eq!(scenario.steps.len(), 2);
        assert_eq!(scenario.steps[0].label(), "POST /customers");
        assert_eq!(scenario.steps[1].label(), "row exists");
        assert!(scenario.uses_db());
    }

    #[test]
    fn a_step_needs_exactly_one_of_http_or_db() {
        let both = r#"{"name":"x","steps":[{"http":{"method":"GET","path":"/"},"db":{"sql":"SELECT 1"}}]}"#;
        assert!(parse_err(both).contains("exactly one"));
        let none = r#"{"name":"x","steps":[{"name":"empty"}]}"#;
        assert!(parse_err(none).contains("step 1"));
    }

    #[test]
    fn mistakes_are_named() {
        let bad_method = r#"{"name":"x","steps":[{"http":{"method":"FETCH","path":"/"}}]}"#;
        assert!(parse_err(bad_method).contains("unsupported HTTP method"));
        let mixed = r#"{"name":"x","steps":[{"db":{"sql":"SELECT 1"},"expect":{"status":200}}]}"#;
        assert!(parse_err(mixed).contains("only apply to `http`"));
        let params = r#"{"name":"x","steps":[{"db":{"sql":"SELECT 1","params":{}}}]}"#;
        assert!(parse_err(params).contains("db.params"));
        let pointer =
            r#"{"name":"x","steps":[{"http":{"method":"GET","path":"/"},"capture":{"a":"id"}}]}"#;
        assert!(parse_err(pointer).contains("not a JSON pointer"));
        let typo = r#"{"name":"x","steps":[{"http":{"method":"GET","path":"/","jsn":{}}}]}"#;
        assert!(parse_err(typo).contains("unknown field"));
    }

    #[test]
    fn entries_are_found_by_stem_or_name() {
        let dir = tempfile::tempdir().expect("tmp");
        fs::write(dir.path().join("create-order.json"), GOOD).expect("write");
        fs::write(dir.path().join("broken.json"), "{").expect("write");
        let entries = load_scenarios(dir.path()).expect("load");
        assert_eq!(entries.len(), 2);
        assert!(entries[0].scenario.is_err());
        assert!(find_scenario(&entries, "Create Order").is_some());
        assert!(find_scenario(&entries, "create_order").is_some());
        assert!(find_scenario(&entries, "missing").is_none());
    }
}
