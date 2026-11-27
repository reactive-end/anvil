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
        }
    }
}

impl WebQaConfig {
    /// URL probed to learn whether the app answers, when `url` is set.
    pub fn ready_url(&self) -> Option<String> {
        let base = self.url.as_deref()?.trim_end_matches('/');
        let path = self.ready_path.trim_start_matches('/');
        Some(if path.is_empty() { format!("{base}/") } else { format!("{base}/{path}") })
    }

    /// The diff threshold clamped to `0.0..=1.0` (NaN falls back to the default).
    pub fn threshold(&self) -> f64 {
        if self.diff_threshold.is_nan() {
            return default_diff_threshold();
        }
        self.diff_threshold.clamp(0.0, 1.0)
    }
}

/// Everything about QA that is not the driver choice.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct QaConfig {
    #[serde(default)]
    pub web: WebQaConfig,
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
    fn threshold_is_clamped() {
        let web = WebQaConfig { diff_threshold: 7.0, ..WebQaConfig::default() };
        assert!((web.threshold() - 1.0).abs() < f64::EPSILON);
        let web = WebQaConfig { diff_threshold: -1.0, ..WebQaConfig::default() };
        assert!(web.threshold().abs() < f64::EPSILON);
    }
}
