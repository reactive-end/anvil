//! Turns a flow into the standalone Playwright script that executes it.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde_json::{Value, json};

use super::spec::{Flow, MAX_WAIT_MS, PressStep, ScreenshotStep, Step};

/// Embedded Playwright flow runner (`scripts/flow_runner.js`).
pub const FLOW_RUNNER_SCRIPT: &str = include_str!("../../scripts/flow_runner.js");

const PLAN_PLACEHOLDER: &str = "__ANVIL_FLOW_PLAN__";

/// Resolves `raw` against `base`; absolute `http(s)` URLs pass through.
pub fn resolve_url(base: Option<&str>, raw: &str) -> Result<String> {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Ok(raw.to_owned());
    }
    match base {
        Some(base) => Ok(format!("{}/{}", base.trim_end_matches('/'), raw.trim_start_matches('/'))),
        None => bail!("relative URL `{raw}` needs `qa.web.url` in .harness/config.json"),
    }
}

/// Builds the runner script for `flow`, writing screenshots under `out_dir`.
pub fn build_flow_script(
    flow: &Flow,
    base_url: Option<&str>,
    out_dir: &Path,
    search_dirs: &[PathBuf],
) -> Result<String> {
    let steps =
        flow.steps.iter().map(|step| plan_step(step, base_url)).collect::<Result<Vec<Value>>>()?;
    let first_is_goto = matches!(flow.steps.first(), Some(Step::Goto(_)));
    let start_url = match (&flow.url, first_is_goto) {
        (Some(url), false) => Some(resolve_url(base_url, url)?),
        _ => None,
    };
    let (_, width, height) = flow.viewport.dimensions();
    let plan = json!({
        "startUrl": start_url,
        "viewport": { "width": width, "height": height },
        "timeoutMs": flow.step_timeout_ms(),
        "outDir": out_dir.to_string_lossy(),
        "searchDirs": search_dirs.iter().map(|d| d.to_string_lossy()).collect::<Vec<_>>(),
        "steps": steps,
    });
    Ok(FLOW_RUNNER_SCRIPT.replacen(PLAN_PLACEHOLDER, &serde_json::to_string(&plan)?, 1))
}

fn plan_step(step: &Step, base_url: Option<&str>) -> Result<Value> {
    Ok(match step {
        Step::Goto(url) => json!({ "kind": "goto", "url": resolve_url(base_url, url)? }),
        Step::WaitFor(selector) => json!({ "kind": "wait_for", "selector": selector }),
        Step::Fill(fill) => {
            json!({ "kind": "fill", "selector": fill.selector, "value": fill.value })
        }
        Step::Select(select) => json!({
            "kind": "select", "selector": select.selector,
            "index": select.index, "value": select.value,
        }),
        Step::Click(selector) => json!({ "kind": "click", "selector": selector }),
        Step::Press(PressStep::Key(key)) => {
            json!({ "kind": "press", "selector": null, "key": key })
        }
        Step::Press(PressStep::On { selector, key }) => {
            json!({ "kind": "press", "selector": selector, "key": key })
        }
        Step::WaitMs(ms) => json!({ "kind": "wait_ms", "ms": (*ms).min(MAX_WAIT_MS) }),
        Step::ExpectRequest(expect) => json!({
            "kind": "expect_request",
            "method": expect.method.as_deref().map(str::to_ascii_uppercase),
            "urlContains": expect.url_contains,
        }),
        Step::ExpectNoRequest(expect) => {
            json!({ "kind": "expect_no_request", "urlContains": expect.url_contains })
        }
        Step::ExpectText(text) => json!({ "kind": "expect_text", "text": text }),
        Step::ExpectVisible(selector) => json!({ "kind": "expect_visible", "selector": selector }),
        Step::ExpectCount(expect) => {
            json!({ "kind": "expect_count", "selector": expect.selector, "count": expect.count })
        }
        Step::Screenshot(shot) => plan_screenshot(shot),
    })
}

fn plan_screenshot(shot: &ScreenshotStep) -> Value {
    match shot {
        ScreenshotStep::Name(name) => json!({
            "kind": "screenshot", "name": name, "selector": null, "mask": [], "fullPage": false,
        }),
        ScreenshotStep::Options(options) => json!({
            "kind": "screenshot", "name": options.name, "selector": options.selector,
            "mask": options.mask, "fullPage": options.full_page,
        }),
    }
}

/// Folders Playwright may be installed in: the repository, `hint`, then app folders below the
/// repository (monorepos keep `node_modules` next to each package).
pub fn playwright_search_dirs(root: &Path, hint: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = vec![root.to_path_buf()];
    if let Some(hint) = hint {
        dirs.push(hint.to_path_buf());
    }
    collect_installs(root, 0, &mut dirs);
    let mut seen = Vec::new();
    dirs.retain(|dir| {
        let fresh = !seen.contains(dir);
        seen.push(dir.clone());
        fresh
    });
    dirs
}

fn collect_installs(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !path.is_dir()
            || name.starts_with('.')
            || matches!(name.as_str(), "node_modules" | "target")
        {
            continue;
        }
        if path.join("node_modules").join("playwright").is_dir() {
            out.push(path.clone());
        }
        if depth < 1 {
            collect_installs(&path, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_resolve_against_the_base() {
        assert_eq!(resolve_url(Some("http://h:1/"), "/a").expect("url"), "http://h:1/a");
        assert_eq!(resolve_url(Some("http://h:1"), "a").expect("url"), "http://h:1/a");
        assert_eq!(resolve_url(None, "https://x/y").expect("url"), "https://x/y");
        assert!(resolve_url(None, "/a").is_err());
    }

    #[test]
    fn script_embeds_the_plan_and_is_valid_javascript() {
        let flow = Flow::parse(
            r##"{"name":"t","url":"/","steps":[
                {"fill":{"selector":"#a","value":"x\"y"}},
                {"wait_ms": 999999},
                {"expect_request":{"method":"post","url_contains":"/o","status":201}},
                {"screenshot":{"name":"s","selector":"#f","mask":["#id"]}}]}"##,
        )
        .expect("flow");
        let dir = tempfile::tempdir().expect("tmp");
        let script =
            build_flow_script(&flow, Some("http://h"), dir.path(), &[dir.path().to_path_buf()])
                .expect("script");
        assert!(!script.contains(PLAN_PLACEHOLDER));
        assert!(script.contains(r#""startUrl":"http://h/""#));
        assert!(script.contains(r#""ms":10000"#));
        assert!(script.contains(r#""method":"POST""#));
        let path = dir.path().join("flow.mjs");
        fs::write(&path, &script).expect("write");
        let check = std::process::Command::new("node").arg("--check").arg(&path).output();
        if let Ok(check) = check {
            assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));
        }
    }

    #[test]
    fn a_leading_goto_replaces_the_start_url() {
        let flow = Flow::parse(r#"{"name":"t","url":"/","steps":[{"goto":"/p"}]}"#).expect("flow");
        let script =
            build_flow_script(&flow, Some("http://h"), Path::new("out"), &[]).expect("script");
        assert!(script.contains(r#""startUrl":null"#));
        assert!(script.contains(r#""url":"http://h/p""#));
    }

    #[test]
    fn search_dirs_find_nested_installs_once() {
        let dir = tempfile::tempdir().expect("tmp");
        let app = dir.path().join("web");
        fs::create_dir_all(app.join("node_modules").join("playwright")).expect("mkdir");
        let found = playwright_search_dirs(dir.path(), Some(&app));
        assert_eq!(found, vec![dir.path().to_path_buf(), app]);
    }
}
