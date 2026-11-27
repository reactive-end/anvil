//! Flow files (`.harness/qa/flows/<name>.json`): parsing and validation.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::web::{DESKTOP_VIEWPORT, MOBILE_VIEWPORT};

/// Longest pause a `wait_ms` step may take; larger values are clamped.
pub const MAX_WAIT_MS: u64 = 10_000;

/// Time a single step may take unless the flow sets `timeout_ms`.
pub const DEFAULT_STEP_TIMEOUT_MS: u64 = 10_000;

/// Browser window a flow runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlowViewport {
    #[default]
    Desktop,
    Mobile,
}

impl FlowViewport {
    /// Name, width, and height of the viewport.
    pub fn dimensions(self) -> (&'static str, u32, u32) {
        match self {
            Self::Desktop => DESKTOP_VIEWPORT,
            Self::Mobile => MOBILE_VIEWPORT,
        }
    }
}

/// One scripted user flow with its expectations.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flow {
    pub name: String,
    /// Page opened before the first step unless that step is a `goto`.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub viewport: FlowViewport,
    /// Per-step timeout in milliseconds.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    pub steps: Vec<Step>,
}

/// A single action or expectation. Selectors are Playwright selectors (CSS, `text=`, `role=`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Goto(String),
    WaitFor(String),
    Fill(FillStep),
    Select(SelectStep),
    Click(String),
    Press(PressStep),
    WaitMs(u64),
    ExpectRequest(RequestExpect),
    ExpectNoRequest(NoRequestExpect),
    ExpectText(String),
    ExpectVisible(String),
    ExpectCount(CountExpect),
    Screenshot(ScreenshotStep),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FillStep {
    pub selector: String,
    pub value: String,
}

/// Picks an option of a `<select>` by position or by value/label.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectStep {
    pub selector: String,
    #[serde(default)]
    pub index: Option<usize>,
    #[serde(default)]
    pub value: Option<String>,
}

/// `"Enter"` presses a key on the page; the object form presses it on one element.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum PressStep {
    Key(String),
    On { selector: String, key: String },
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestExpect {
    #[serde(default)]
    pub method: Option<String>,
    pub url_contains: String,
    #[serde(default)]
    pub status: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoRequestExpect {
    pub url_contains: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountExpect {
    pub selector: String,
    pub count: usize,
}

/// `"name"` captures the viewport; the object form can target one element and mask dynamic parts.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ScreenshotStep {
    Name(String),
    Options(ScreenshotOptions),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotOptions {
    pub name: String,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub mask: Vec<String>,
    #[serde(default)]
    pub full_page: bool,
}

impl ScreenshotStep {
    /// File-safe name of the screenshot (and of its baseline).
    pub fn name(&self) -> &str {
        match self {
            Self::Name(name) => name,
            Self::Options(options) => &options.name,
        }
    }
}

impl Flow {
    /// Parses and validates a flow file's text.
    pub fn parse(raw: &str) -> Result<Self> {
        let clean = raw.strip_prefix('\u{feff}').unwrap_or(raw);
        let flow: Self = serde_json::from_str(clean).context("invalid flow JSON")?;
        flow.validate()?;
        Ok(flow)
    }

    /// Per-step timeout in milliseconds.
    pub fn step_timeout_ms(&self) -> u64 {
        self.timeout_ms.unwrap_or(DEFAULT_STEP_TIMEOUT_MS).max(100)
    }

    /// Checks everything serde cannot: required text, ranges, and unique screenshot names.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("flow `name` must not be empty");
        }
        if self.steps.is_empty() {
            bail!("flow `{}` has no steps", self.name);
        }
        let mut shots = HashSet::new();
        for (idx, step) in self.steps.iter().enumerate() {
            step.validate().with_context(|| format!("step {} ({})", idx + 1, step.describe()))?;
            if let Step::Screenshot(shot) = step
                && !shots.insert(shot.name().to_owned())
            {
                bail!("step {}: screenshot name `{}` is used twice", idx + 1, shot.name());
            }
        }
        Ok(())
    }
}

impl Step {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Goto(text)
            | Self::WaitFor(text)
            | Self::Click(text)
            | Self::ExpectText(text)
            | Self::ExpectVisible(text) => require_text(text),
            Self::Fill(fill) => require_text(&fill.selector),
            Self::Select(select) => select.validate(),
            Self::Press(PressStep::Key(key)) => require_text(key),
            Self::Press(PressStep::On { selector, key }) => {
                require_text(selector)?;
                require_text(key)
            }
            Self::WaitMs(_) => Ok(()),
            Self::ExpectRequest(expect) => expect.validate(),
            Self::ExpectNoRequest(expect) => require_text(&expect.url_contains),
            Self::ExpectCount(expect) => require_text(&expect.selector),
            Self::Screenshot(shot) => validate_screenshot(shot),
        }
    }

    /// Short human-readable form used in reports.
    pub fn describe(&self) -> String {
        match self {
            Self::Goto(url) => format!("goto {url}"),
            Self::WaitFor(sel) => format!("wait_for `{sel}`"),
            Self::Fill(fill) => format!("fill `{}`", fill.selector),
            Self::Select(select) => format!("select `{}`", select.selector),
            Self::Click(sel) => format!("click `{sel}`"),
            Self::Press(PressStep::Key(key)) => format!("press {key}"),
            Self::Press(PressStep::On { selector, key }) => format!("press {key} on `{selector}`"),
            Self::WaitMs(ms) => format!("wait_ms {ms}"),
            Self::ExpectRequest(expect) => expect.describe(),
            Self::ExpectNoRequest(expect) => {
                format!("expect_no_request `{}`", expect.url_contains)
            }
            Self::ExpectText(text) => format!("expect_text `{text}`"),
            Self::ExpectVisible(sel) => format!("expect_visible `{sel}`"),
            Self::ExpectCount(expect) => {
                format!("expect_count `{}` == {}", expect.selector, expect.count)
            }
            Self::Screenshot(shot) => format!("screenshot {}", shot.name()),
        }
    }
}

impl SelectStep {
    fn validate(&self) -> Result<()> {
        require_text(&self.selector)?;
        match (&self.index, &self.value) {
            (Some(_), None) | (None, Some(_)) => Ok(()),
            _ => bail!("`select` needs exactly one of `index` or `value`"),
        }
    }
}

impl RequestExpect {
    fn validate(&self) -> Result<()> {
        require_text(&self.url_contains)?;
        if let Some(method) = &self.method
            && !matches!(
                method.to_ascii_uppercase().as_str(),
                "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
            )
        {
            bail!("unsupported HTTP method `{method}`");
        }
        if let Some(status) = self.status
            && !(100..=599).contains(&status)
        {
            bail!("status {status} is not an HTTP status code");
        }
        Ok(())
    }

    /// Request-expectation wording shared by reports and failure reasons.
    pub fn describe(&self) -> String {
        let method = self
            .method
            .as_deref()
            .map_or_else(String::new, |m| format!("{} ", m.to_ascii_uppercase()));
        let status = self.status.map_or_else(String::new, |s| format!(" -> {s}"));
        format!("expect_request {method}*{}*{status}", self.url_contains)
    }
}

fn validate_screenshot(shot: &ScreenshotStep) -> Result<()> {
    let name = shot.name();
    let safe =
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !safe {
        bail!("screenshot name `{name}` may only contain letters, digits, `-`, and `_`");
    }
    Ok(())
}

fn require_text(text: &str) -> Result<()> {
    if text.trim().is_empty() {
        bail!("value must not be empty");
    }
    Ok(())
}

/// A flow file found on disk, valid or not.
#[derive(Debug)]
pub struct FlowEntry {
    /// File stem; names the flow on the command line and its baseline folder.
    pub id: String,
    pub path: PathBuf,
    pub flow: std::result::Result<Flow, String>,
}

/// Reads every `*.json` file of `dir` (sorted by name); a missing directory has no flows.
pub fn load_flows(dir: &Path) -> Result<Vec<FlowEntry>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("failed to read {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "json") && p.is_file())
        .collect();
    paths.sort();
    Ok(paths.into_iter().map(read_entry).collect())
}

fn read_entry(path: PathBuf) -> FlowEntry {
    let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let flow = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))
        .and_then(|raw| Flow::parse(&raw).map_err(|err| format!("{err:#}")));
    FlowEntry { id, path, flow }
}

/// Picks the flow whose file stem or `name` equals `wanted` (ignoring case, spaces, and `-`/`_`).
pub fn find_flow<'a>(entries: &'a [FlowEntry], wanted: &str) -> Option<&'a FlowEntry> {
    let key = |text: &str| {
        text.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
    };
    let wanted = key(wanted);
    entries.iter().find(|entry| {
        key(&entry.id) == wanted || entry.flow.as_ref().is_ok_and(|flow| key(&flow.name) == wanted)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREATE: &str = r##"{ "name": "create order", "url": "/", "viewport": "desktop",
      "steps": [
        { "goto": "/" },
        { "wait_for": "table tbody tr" },
        { "fill": { "selector": "#item", "value": "Widget" } },
        { "select": { "selector": "#customer", "index": 1 } },
        { "click": "button[type=submit]" },
        { "press": "Enter" },
        { "wait_ms": 50 },
        { "expect_request": { "method": "POST", "url_contains": "/orders", "status": 201 } },
        { "expect_no_request": { "url_contains": "/customers" } },
        { "expect_text": "Widget" },
        { "expect_visible": "table tbody tr" },
        { "expect_count": { "selector": "tbody tr", "count": 4 } },
        { "screenshot": "after-create" }
      ] }"##;

    #[test]
    fn parses_every_step_kind() {
        let flow = Flow::parse(CREATE).expect("parse");
        assert_eq!(flow.steps.len(), 13);
        assert_eq!(flow.viewport, FlowViewport::Desktop);
        assert_eq!(flow.step_timeout_ms(), DEFAULT_STEP_TIMEOUT_MS);
        assert!(matches!(flow.steps[7], Step::ExpectRequest(_)));
    }

    #[test]
    fn unknown_step_is_rejected_with_the_valid_names() {
        let err = Flow::parse(r#"{"name":"x","steps":[{"hover":"a"}]}"#).unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("unknown variant `hover`"), "{text}");
        assert!(text.contains("expect_request"), "{text}");
    }

    #[test]
    fn unknown_keys_are_rejected_at_every_level() {
        assert!(Flow::parse(r#"{"name":"x","steps":[{"goto":"/"}],"extra":1}"#).is_err());
        let bad = r#"{"name":"x","steps":[{"expect_request":{"url_contains":"/a","stauts":1}}]}"#;
        assert!(Flow::parse(bad).is_err());
    }

    #[test]
    fn validation_catches_bad_values() {
        let cases = [
            (r#"{"name":"","steps":[{"goto":"/"}]}"#, "name"),
            (r#"{"name":"x","steps":[]}"#, "no steps"),
            (r#"{"name":"x","steps":[{"click":" "}]}"#, "must not be empty"),
            (r#"{"name":"x","steps":[{"select":{"selector":"s"}}]}"#, "exactly one"),
            (r#"{"name":"x","steps":[{"screenshot":"a/b"}]}"#, "may only contain"),
            (r#"{"name":"x","steps":[{"screenshot":"a"},{"screenshot":"a"}]}"#, "used twice"),
            (
                r#"{"name":"x","steps":[{"expect_request":{"url_contains":"/a","status":99}}]}"#,
                "not an HTTP status",
            ),
            (
                r#"{"name":"x","steps":[{"expect_request":{"url_contains":"/a","method":"FETCH"}}]}"#,
                "unsupported HTTP method",
            ),
        ];
        for (raw, needle) in cases {
            let text = format!("{:#}", Flow::parse(raw).unwrap_err());
            assert!(text.contains(needle), "`{needle}` not in `{text}`");
        }
    }

    #[test]
    fn flows_load_sorted_and_find_by_stem_or_name() {
        let dir = tempfile::tempdir().expect("tmp");
        fs::write(dir.path().join("b-create-order.json"), CREATE).expect("write");
        fs::write(dir.path().join("a-broken.json"), "{").expect("write");
        fs::write(dir.path().join("notes.txt"), "x").expect("write");
        let entries = load_flows(dir.path()).expect("load");
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["a-broken", "b-create-order"]);
        assert!(entries[0].flow.is_err());
        assert_eq!(
            find_flow(&entries, "Create Order").map(|e| e.id.as_str()),
            Some("b-create-order")
        );
        assert_eq!(
            find_flow(&entries, "b_create_order").map(|e| e.id.as_str()),
            Some("b-create-order")
        );
        assert!(find_flow(&entries, "missing").is_none());
        assert!(load_flows(&dir.path().join("nope")).expect("none").is_empty());
    }
}
