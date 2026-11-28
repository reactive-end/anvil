//! `qa` section of `.harness/config.json`: how Anvil reaches and exercises the web app under test.

use serde::{Deserialize, Serialize};

/// Web app under test: where it runs, how to start it, and where its QA flows live.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebQaConfig {
    /// Base URL of the running app; relative flow URLs resolve against it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Shell command that starts the app (it must keep running until stopped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_command: Option<String>,
    /// Directory (relative to the repository root) `start_command` runs in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_cwd: Option<String>,
    /// Path appended to `url` to learn whether the app answers.
    #[serde(default = "default_ready_path")]
    pub ready_path: String,
    /// Seconds to wait for the started app to answer.
    #[serde(default = "default_ready_timeout_secs")]
    pub ready_timeout_secs: u64,
    /// Directory (relative to the repository root) holding the flow JSON files.
    #[serde(default = "default_flows_dir")]
    pub flows_dir: String,
    /// Share of changed pixels (0.0 to 1.0) above which a screenshot counts as a visual diff.
    #[serde(default = "default_diff_threshold")]
    pub diff_threshold: f64,
    /// Let Tier 2 start the app itself when it is not running.
    #[serde(default)]
    pub autostart: bool,
    /// Path prefixes whose changes (any file, not only markup and styles) trigger the flows;
    /// `start_cwd` when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

impl Eq for WebQaConfig {}

impl Default for WebQaConfig {
    fn default() -> Self {
        Self {
            url: None,
            start_command: None,
            start_cwd: None,
            ready_path: default_ready_path(),
            ready_timeout_secs: default_ready_timeout_secs(),
            flows_dir: default_flows_dir(),
            diff_threshold: default_diff_threshold(),
            autostart: false,
            paths: Vec::new(),
        }
    }
}

impl WebQaConfig {
    /// `true` when one of the `changed` files is under `paths` (else `start_cwd`); with neither
    /// set, any change counts.
    pub fn touches(&self, changed: &[String]) -> bool {
        touches_watched(&self.paths, self.start_cwd.as_deref(), changed)
    }

    /// URL probed to learn whether the app answers, when `url` is set.
    pub fn ready_url(&self) -> Option<String> {
        join_ready_url(self.url.as_deref()?, &self.ready_path)
    }

    /// The diff threshold clamped to `0.0..=1.0` (NaN falls back to the default).
    pub fn threshold(&self) -> f64 {
        if self.diff_threshold.is_nan() {
            return default_diff_threshold();
        }
        self.diff_threshold.clamp(0.0, 1.0)
    }
}

/// API under test: where it runs, how to start it, and where its scenario files live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiQaConfig {
    /// Base URL of the running API; relative scenario paths resolve against it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Shell command that starts the API (it must keep running until stopped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_command: Option<String>,
    /// Directory (relative to the repository root) `start_command` runs in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_cwd: Option<String>,
    /// Path appended to `url` to learn whether the API answers.
    #[serde(default = "default_ready_path", skip_serializing_if = "is_default_ready_path")]
    pub ready_path: String,
    /// Seconds to wait for the started API to answer.
    #[serde(
        default = "default_ready_timeout_secs",
        skip_serializing_if = "is_default_ready_timeout"
    )]
    pub ready_timeout_secs: u64,
    /// Directory (relative to the repository root) holding the scenario JSON files.
    #[serde(default = "default_scenarios_dir", skip_serializing_if = "is_default_scenarios_dir")]
    pub scenarios_dir: String,
    /// Let Tier 2 start the API itself when it is not running.
    #[serde(default, skip_serializing_if = "is_false")]
    pub autostart: bool,
    /// Path prefixes whose changes trigger the Tier 2 scenarios; `start_cwd` when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

impl Default for ApiQaConfig {
    fn default() -> Self {
        Self {
            url: None,
            start_command: None,
            start_cwd: None,
            ready_path: default_ready_path(),
            ready_timeout_secs: default_ready_timeout_secs(),
            scenarios_dir: default_scenarios_dir(),
            autostart: false,
            paths: Vec::new(),
        }
    }
}

impl ApiQaConfig {
    /// `true` when nothing was configured.
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    /// URL probed to learn whether the API answers, when `url` is set.
    pub fn ready_url(&self) -> Option<String> {
        join_ready_url(self.url.as_deref()?, &self.ready_path)
    }

    /// Prefixes whose changes should trigger the scenarios: `paths`, else `start_cwd`.
    /// Empty means the configuration cannot narrow it down, so any change triggers them.
    pub fn watched_paths(&self) -> Vec<&str> {
        watched(&self.paths, self.start_cwd.as_deref())
    }

    /// `true` when one of the `changed` files (repo-relative) is under a watched prefix.
    pub fn touches(&self, changed: &[String]) -> bool {
        touches_watched(&self.paths, self.start_cwd.as_deref(), changed)
    }
}

fn watched<'a>(paths: &'a [String], start_cwd: Option<&'a str>) -> Vec<&'a str> {
    if paths.is_empty() {
        return start_cwd.into_iter().collect();
    }
    paths.iter().map(String::as_str).collect()
}

fn touches_watched(paths: &[String], start_cwd: Option<&str>, changed: &[String]) -> bool {
    let watched = watched(paths, start_cwd);
    if watched.is_empty() {
        return !changed.is_empty();
    }
    let prefixes: Vec<String> = watched.iter().map(|p| normalize_prefix(p)).collect();
    changed.iter().map(|c| normalize_prefix(c)).any(|file| {
        prefixes.iter().any(|prefix| {
            prefix.is_empty()
                || file == *prefix
                || file.strip_prefix(prefix.as_str()).is_some_and(|rest| rest.starts_with('/'))
        })
    })
}

fn normalize_prefix(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let trimmed = unified.trim_start_matches("./").trim_matches('/');
    if trimmed == "." { String::new() } else { trimmed.to_owned() }
}

fn join_ready_url(url: &str, ready_path: &str) -> Option<String> {
    let base = url.trim_end_matches('/');
    let path = ready_path.trim_start_matches('/');
    Some(if path.is_empty() { format!("{base}/") } else { format!("{base}/{path}") })
}

/// Everything about QA that is not the driver choice.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct QaConfig {
    #[serde(default)]
    pub web: WebQaConfig,
    /// API scenarios (HTTP calls plus database checks); left out of the file when unset.
    #[serde(default, skip_serializing_if = "ApiQaConfig::is_default")]
    pub api: ApiQaConfig,
}

impl QaConfig {
    /// `true` when nothing was configured, so the section is left out of the saved file.
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

fn default_ready_path() -> String {
    "/".to_owned()
}

fn default_ready_timeout_secs() -> u64 {
    60
}

fn default_scenarios_dir() -> String {
    ".harness/qa/api".to_owned()
}

fn is_default_ready_path(path: &String) -> bool {
    path == "/"
}

fn is_default_ready_timeout(secs: &u64) -> bool {
    *secs == 60
}

fn is_default_scenarios_dir(dir: &String) -> bool {
    dir == ".harness/qa/api"
}

fn is_false(flag: &bool) -> bool {
    !flag
}

fn default_flows_dir() -> String {
    ".harness/qa/flows".to_owned()
}

fn default_diff_threshold() -> f64 {
    0.01
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_to_an_empty_section() {
        let parsed: QaConfig = serde_json::from_str("{}").expect("parse");
        assert!(parsed.is_default());
        assert_eq!(parsed.web.ready_path, "/");
        assert_eq!(parsed.web.ready_timeout_secs, 60);
        assert_eq!(parsed.web.flows_dir, ".harness/qa/flows");
        assert!((parsed.web.diff_threshold - 0.01).abs() < f64::EPSILON);
        assert!(!parsed.web.autostart);
    }

    #[test]
    fn ready_url_joins_base_and_path_with_one_slash() {
        let web = WebQaConfig {
            url: Some("http://localhost:5180/".to_owned()),
            ready_path: "health".to_owned(),
            ..WebQaConfig::default()
        };
        assert_eq!(web.ready_url().as_deref(), Some("http://localhost:5180/health"));
        assert_eq!(WebQaConfig::default().ready_url(), None);
    }

    #[test]
    fn api_section_is_left_out_of_the_file_until_configured() {
        let json = serde_json::to_value(QaConfig::default()).expect("serialize");
        assert!(json.get("api").is_none(), "{json}");
        let parsed: QaConfig =
            serde_json::from_str(r#"{"api":{"url":"http://localhost:3100"}}"#).expect("parse");
        assert_eq!(parsed.api.ready_path, "/");
        assert_eq!(parsed.api.scenarios_dir, ".harness/qa/api");
        let saved = serde_json::to_value(&parsed).expect("serialize");
        assert_eq!(saved["api"], serde_json::json!({ "url": "http://localhost:3100" }));
    }

    #[test]
    fn api_paths_fall_back_to_the_start_directory() {
        let changed = |files: &[&str]| files.iter().map(|f| (*f).to_owned()).collect::<Vec<_>>();
        let api =
            ApiQaConfig { start_cwd: Some("orders-api".to_owned()), ..ApiQaConfig::default() };
        assert!(api.touches(&changed(&["orders-api/src/main.ts"])));
        assert!(api.touches(&changed(&["orders-api\\src\\main.ts"])));
        assert!(!api.touches(&changed(&["orders-api-docs/readme.md", "orders-web/a.ts"])));
        let explicit = ApiQaConfig { paths: vec!["./services/".to_owned()], ..api };
        assert!(explicit.touches(&changed(&["services/a.ts"])));
        assert!(!explicit.touches(&changed(&["orders-api/src/main.ts"])));
        assert!(ApiQaConfig::default().touches(&changed(&["anything"])));
        assert!(!ApiQaConfig::default().touches(&[]));
    }

    #[test]
    fn threshold_is_clamped() {
        let web = WebQaConfig { diff_threshold: 7.0, ..WebQaConfig::default() };
        assert!((web.threshold() - 1.0).abs() < f64::EPSILON);
        let web = WebQaConfig { diff_threshold: -1.0, ..WebQaConfig::default() };
        assert!(web.threshold().abs() < f64::EPSILON);
    }
}
