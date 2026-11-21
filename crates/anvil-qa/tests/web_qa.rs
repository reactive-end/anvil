use std::path::PathBuf;
use std::process::Command;

use anvil_qa::QaEngine;
use anvil_qa::model::{QaViolationKind, UiPlatform};
use anvil_qa::report::ReportBudget;
use anvil_qa::web::{
    DOM_AUDIT_SCRIPT, WebEvalInput, build_cdp_evaluate_payload, build_playwright_runner_script,
};

fn run_dom_audit_in_node(fixture_js: &str) -> String {
    let script_literal = serde_json::to_string(DOM_AUDIT_SCRIPT).expect("serialize script");
    let harness = format!(
        r#"
const vm = require("node:vm");
{fixture_js}
const sandbox = {{
  window: globalThis.__mockWindow,
  document: globalThis.__mockDocument,
  getComputedStyle: globalThis.__mockGetComputedStyle,
  Set, Array, Math, String, Number, Boolean, Object, JSON,
}};
sandbox.globalThis = sandbox;
const result = vm.runInNewContext({script_literal}, sandbox);
process.stdout.write(JSON.stringify(result));
"#
    );

    let output = Command::new("node")
        .arg("-e")
        .arg(harness)
        .output()
        .expect("node must execute dom_audit.js fixture");
    assert!(
        output.status.success(),
        "node failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("valid utf8 from node")
}

const FIVE_VIOLATION_FIXTURE_JS: &str = r#"
function makeEl(opts) {
  const el = {
    tagName: opts.tagName,
    id: opts.id || "",
    className: opts.className || "",
    type: opts.type || "",
    src: opts.src || "",
    complete: Boolean(opts.complete),
    naturalWidth: opts.naturalWidth ?? 100,
    scrollWidth: opts.scrollWidth ?? 100,
    clientWidth: opts.clientWidth ?? 100,
    scrollHeight: opts.scrollHeight ?? 20,
    clientHeight: opts.clientHeight ?? 20,
    parentElement: opts.parentElement || null,
    children: opts.children || [],
    childNodes: opts.text ? [{ nodeType: 3, nodeValue: opts.text }] : [],
    textContent: opts.text || "",
    _style: opts.style || {},
    _rect: opts.rect || { left: 0, top: 0, width: 100, height: 20, right: 100, bottom: 20 },
    getBoundingClientRect() { return this._rect; },
    contains(other) {
      let cur = other;
      while (cur) { if (cur === this) return true; cur = cur.parentElement; }
      return false;
    }
  };
  return el;
}
const root = makeEl({ tagName: "HTML", scrollWidth: 1620, scrollHeight: 900 });
const scrollParent = makeEl({
  tagName: "DIV", id: "scroller", parentElement: root,
  style: { overflowX: "auto", overflowY: "auto" },
  rect: { left: 0, top: 200, width: 120, height: 40, right: 120, bottom: 240 },
});
const okScrollText = makeEl({
  tagName: "SPAN", id: "ok-scroll", text: "Inside scrollable container",
  parentElement: scrollParent, scrollWidth: 300, clientWidth: 100,
  rect: { left: 0, top: 200, width: 100, height: 20, right: 100, bottom: 220 },
});
const okEllipsis = makeEl({
  tagName: "SPAN", id: "ok-ellipsis", text: "Truncated intentionally",
  parentElement: root, scrollWidth: 250, clientWidth: 100,
  style: { overflowX: "hidden", textOverflow: "ellipsis" },
  rect: { left: 0, top: 250, width: 100, height: 20, right: 100, bottom: 270 },
});
const clippedText = makeEl({
  tagName: "P", id: "bad-clip", text: "Clipped without scroll or ellipsis",
  parentElement: root, scrollWidth: 260, clientWidth: 100,
  style: { overflowX: "hidden" },
  rect: { left: 10, top: 10, width: 100, height: 20, right: 110, bottom: 30 },
});
const btnA = makeEl({
  tagName: "BUTTON", id: "save-btn", text: "Save", parentElement: root,
  rect: { left: 20, top: 50, width: 120, height: 40, right: 140, bottom: 90 },
});
const btnB = makeEl({
  tagName: "BUTTON", id: "cancel-btn", text: "Cancel", parentElement: root,
  rect: { left: 100, top: 60, width: 120, height: 40, right: 220, bottom: 100 },
});
const brokenImg = makeEl({
  tagName: "IMG", id: "hero-img", src: "/missing.png", complete: true, naturalWidth: 0,
  parentElement: root,
  rect: { left: 0, top: 120, width: 80, height: 80, right: 80, bottom: 200 },
});
const elements = [root, scrollParent, okScrollText, okEllipsis, clippedText, btnA, btnB, brokenImg];
globalThis.__mockWindow = { innerWidth: 1440, innerHeight: 900 };
globalThis.__mockGetComputedStyle = (el) => el._style || {};
globalThis.__mockDocument = {
  documentElement: root,
  querySelectorAll(sel) {
    if (sel === "img") return elements.filter((e) => e.tagName === "IMG");
    if (sel.includes("button")) return elements.filter((e) => ["BUTTON", "A", "INPUT", "SELECT", "TEXTAREA"].includes(e.tagName));
    return elements;
  }
};
"#;

#[test]
fn dom_audit_js_and_web_rs_detect_all_five_deliberate_violations() {
    let dom_json = run_dom_audit_in_node(FIVE_VIOLATION_FIXTURE_JS);
    let engine = QaEngine::new(".");
    let capture = engine
        .evaluate_web_capture(WebEvalInput {
            viewport_name: "desktop",
            width: 1440,
            height: 900,
            dom_json: &dom_json,
            console_errors: vec!["Uncaught TypeError: undefined is not a function".to_owned()],
            page_errors: vec![],
            screenshot_path: Some(PathBuf::from("before-desktop.png")),
        })
        .expect("evaluate web capture");

    assert!(!capture.passed());
    assert_eq!(capture.total_violations(), 5);

    let kinds: Vec<QaViolationKind> = capture.violations.iter().map(|v| v.kind).collect();
    assert!(kinds.contains(&QaViolationKind::HorizontalOverflow));
    assert!(kinds.contains(&QaViolationKind::ClippedText));
    assert!(kinds.contains(&QaViolationKind::OverlappingInteractive));
    assert!(kinds.contains(&QaViolationKind::BrokenImage));
    assert!(kinds.contains(&QaViolationKind::ConsoleError));

    let clean_capture = engine
        .evaluate_web_capture(WebEvalInput {
            viewport_name: "desktop",
            width: 1440,
            height: 900,
            dom_json: r#"{"passed":true,"violations":[]}"#,
            console_errors: vec![],
            page_errors: vec![],
            screenshot_path: Some(PathBuf::from("after-desktop.png")),
        })
        .expect("evaluate clean capture");

    let report = engine.finalize_report(
        UiPlatform::Web,
        vec![capture],
        vec![clean_capture],
        vec![],
        vec![],
    );
    assert!(report.passed);
    let summary = engine.format_report(&report, ReportBudget::default());
    assert!(summary.contains("QA [PASS] platform=web web_violations=0 (before=5)"));
    assert!(summary.contains("before=5 -> after=0 [PASS]"));
}

#[test]
fn dom_audit_js_detects_zero_size_control_and_builders_are_valid() {
    let fixture = r#"
const root = { tagName: "HTML", scrollWidth: 1440, scrollHeight: 900 };
const zeroBtn = {
  tagName: "BUTTON", id: "collapsed", className: "cta",
  parentElement: root, children: [], childNodes: [], textContent: "",
  _style: { display: "inline-block", visibility: "visible" },
  _rect: { left: 0, top: 0, width: 0, height: 32, right: 0, bottom: 32 },
  getBoundingClientRect() { return this._rect; },
  contains() { return false; }
};
globalThis.__mockWindow = { innerWidth: 1440, innerHeight: 900 };
globalThis.__mockGetComputedStyle = (el) => el._style || {};
globalThis.__mockDocument = {
  documentElement: root,
  querySelectorAll(sel) {
    if (sel.includes("button")) return [zeroBtn];
    if (sel === "img") return [];
    return [root, zeroBtn];
  }
};
"#;

    let dom_json = run_dom_audit_in_node(fixture);
    let engine = QaEngine::new(".");
    let capture = engine
        .evaluate_web_capture(WebEvalInput {
            viewport_name: "mobile",
            width: 390,
            height: 844,
            dom_json: &dom_json,
            console_errors: vec![],
            page_errors: vec![],
            screenshot_path: None,
        })
        .expect("evaluate zero-size capture");

    assert_eq!(capture.violations.len(), 1);
    assert_eq!(capture.violations[0].kind, QaViolationKind::ZeroSizeControl);

    let cdp = build_cdp_evaluate_payload();
    assert_eq!(cdp["method"], "Runtime.evaluate");
    assert_eq!(cdp["params"]["returnByValue"], true);

    let runner = build_playwright_runner_script(
        "http://127.0.0.1:4173",
        "after",
        PathBuf::from(".harness/.state/qa").as_path(),
    )
    .expect("build playwright script");
    assert!(runner.contains("1440"));
    assert!(runner.contains("390"));
}
