use std::fs;
use std::path::PathBuf;
use std::process::Command;

use tempfile::tempdir;

use anvil_qa::QaEngine;
use anvil_qa::model::QaViolationKind;
use anvil_qa::web::{DOM_AUDIT_SCRIPT, WebEvalInput, build_playwright_runner_script};

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
    assert!(output.status.success(), "node failed: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("valid utf8 from node")
}

const HIDDEN_ANCESTOR_FIXTURE_JS: &str = r#"
function el(opts) {
  return {
    tagName: opts.tagName,
    id: opts.id || "",
    className: "",
    hidden: Boolean(opts.hidden),
    parentElement: opts.parentElement || null,
    children: [],
    childNodes: [],
    textContent: "",
    attributes: opts.attributes || {},
    _style: opts.style || {},
    _rect: opts.rect || { left: 0, top: 0, width: 0, height: 0, right: 0, bottom: 0 },
    getAttribute(k) { return this.attributes[k] ?? null; },
    hasAttribute(k) { return Object.prototype.hasOwnProperty.call(this.attributes, k); },
    getBoundingClientRect() { return this._rect; },
    contains(other) {
      let c = other;
      while (c) { if (c === this) return true; c = c.parentElement; }
      return false;
    }
  };
}
const root = el({ tagName: "HTML" });
const collapsedMenu = el({ tagName: "DIV", id: "menu", parentElement: root, style: { display: "none" } });
const hiddenParent = el({ tagName: "DIV", id: "vis", parentElement: root, style: { visibility: "hidden" } });
const fadedParent = el({ tagName: "DIV", id: "fade", parentElement: root, style: { opacity: "0" } });
const attrHiddenParent = el({ tagName: "DIV", id: "attr", parentElement: root, attributes: { hidden: "" } });

const b1 = el({ tagName: "BUTTON", id: "in-none", parentElement: collapsedMenu });
const b2 = el({ tagName: "BUTTON", id: "in-vis", parentElement: hiddenParent });
const b3 = el({ tagName: "BUTTON", id: "in-opacity", parentElement: fadedParent });
const b4 = el({ tagName: "BUTTON", id: "in-attr", parentElement: attrHiddenParent });
const all = [root, collapsedMenu, hiddenParent, fadedParent, attrHiddenParent, b1, b2, b3, b4];
globalThis.__mockWindow = { innerWidth: 1440, innerHeight: 900 };
globalThis.__mockGetComputedStyle = (node) => node._style || {};
globalThis.__mockDocument = {
  documentElement: root,
  querySelectorAll(sel) {
    if (sel.includes("button")) return [b1, b2, b3, b4];
    if (sel === "img") return [];
    return all;
  }
};
"#;

const MODAL_OVERLAP_FIXTURE_JS: &str = r#"
function el(opts) {
  return {
    tagName: opts.tagName,
    id: opts.id || "",
    className: "",
    parentElement: opts.parentElement || null,
    children: [],
    childNodes: [],
    textContent: "",
    attributes: opts.attributes || {},
    _style: opts.style || {},
    _rect: opts.rect || { left: 0, top: 0, width: 100, height: 40, right: 100, bottom: 40 },
    getAttribute(k) { return this.attributes[k] ?? null; },
    hasAttribute(k) { return Object.prototype.hasOwnProperty.call(this.attributes, k); },
    getBoundingClientRect() { return this._rect; },
    contains(other) {
      let c = other;
      while (c) { if (c === this) return true; c = c.parentElement; }
      return false;
    }
  };
}
const root = el({ tagName: "HTML" });
const bgBtn = el({
  tagName: "BUTTON", id: "bg-action", parentElement: root,
  rect: { left: 100, top: 100, width: 140, height: 40, right: 240, bottom: 140 }
});
const modal = el({
  tagName: "DIV", id: "confirm-modal", parentElement: root,
  attributes: { role: "dialog", "aria-modal": "true" },
  style: { position: "fixed" }
});
const modalOk = el({
  tagName: "BUTTON", id: "modal-ok", parentElement: modal,
  rect: { left: 110, top: 105, width: 120, height: 40, right: 230, bottom: 145 }
});
const fixedFab = el({
  tagName: "BUTTON", id: "fab", parentElement: root,
  style: { position: "fixed" },
  rect: { left: 105, top: 110, width: 80, height: 40, right: 185, bottom: 150 }
});
const modal2 = el({
  tagName: "DIV", id: "second-modal", parentElement: root,
  attributes: { "aria-modal": "true" }
});
const m2BtnA = el({
  tagName: "BUTTON", id: "m2-a", parentElement: modal2,
  rect: { left: 400, top: 400, width: 100, height: 40, right: 500, bottom: 440 }
});
const m2BtnB = el({
  tagName: "BUTTON", id: "m2-b", parentElement: modal2,
  rect: { left: 450, top: 410, width: 100, height: 40, right: 550, bottom: 450 }
});

const all = [root, bgBtn, modal, modalOk, fixedFab, modal2, m2BtnA, m2BtnB];
globalThis.__mockWindow = { innerWidth: 1440, innerHeight: 900 };
globalThis.__mockGetComputedStyle = (node) => node._style || {};
globalThis.__mockDocument = {
  documentElement: root,
  querySelectorAll(sel) {
    if (sel.includes("button")) return [bgBtn, modalOk, fixedFab, m2BtnA, m2BtnB];
    if (sel === "img") return [];
    return all;
  }
};
"#;

#[test]
fn dom_audit_skips_controls_inside_hidden_ancestors() {
    let dom_json = run_dom_audit_in_node(HIDDEN_ANCESTOR_FIXTURE_JS);
    let capture = QaEngine::new(".")
        .evaluate_web_capture(WebEvalInput {
            viewport_name: "desktop",
            width: 1440,
            height: 900,
            dom_json: &dom_json,
            console_errors: vec![],
            page_errors: vec![],
            screenshot_path: None,
        })
        .expect("evaluate hidden ancestors capture");

    assert!(
        capture.violations.is_empty(),
        "expected no false positives for hidden ancestors, got {:?}",
        capture.violations
    );
}

#[test]
fn dom_audit_ignores_modal_vs_background_overlap_but_catches_same_modal_overlap() {
    let dom_json = run_dom_audit_in_node(MODAL_OVERLAP_FIXTURE_JS);
    let capture = QaEngine::new(".")
        .evaluate_web_capture(WebEvalInput {
            viewport_name: "desktop",
            width: 1440,
            height: 900,
            dom_json: &dom_json,
            console_errors: vec![],
            page_errors: vec![],
            screenshot_path: None,
        })
        .expect("evaluate modal overlap capture");

    assert_eq!(capture.violations.len(), 1);
    assert_eq!(capture.violations[0].kind, QaViolationKind::OverlappingInteractive);
    assert!(capture.violations[0].selector.contains("button#m2-a"));
    assert!(capture.violations[0].selector.contains("button#m2-b"));
}

#[test]
fn playwright_rs_and_generated_script_contain_zero_eval_and_pass_node_check() {
    let playwright_rs = include_str!("../src/web/playwright.rs");
    assert!(!playwright_rs.contains("eval("), "playwright.rs must not use eval() for CSP safety");

    let dir = tempdir().expect("tempdir");
    let script = build_playwright_runner_script("http://127.0.0.1:4173", "before", dir.path())
        .expect("build playwright script");
    assert!(!script.contains("eval("));
    assert!(script.contains("await page.evaluate(runDomAudit)"));

    let script_path = PathBuf::from(dir.path()).join("runner.mjs");
    fs::write(&script_path, script).expect("write runner.mjs");
    let check =
        Command::new("node").arg("--check").arg(&script_path).output().expect("node --check");
    assert!(
        check.status.success(),
        "node --check failed on generated playwright script: {}",
        String::from_utf8_lossy(&check.stderr)
    );
}

const DEEP_HIDDEN_TREE_FIXTURE_JS: &str = r#"
function el(opts) {
  return {
    tagName: opts.tagName, id: opts.id || "", className: "",
    parentElement: opts.parentElement || null, children: [],
    childNodes: opts.text ? [{ nodeType: 3, nodeValue: opts.text }] : [],
    textContent: opts.text || "", attributes: {}, _style: opts.style || {},
    _rect: { left: 0, top: 0, width: 100, height: 20, right: 100, bottom: 20 },
    scrollWidth: 300, clientWidth: 100,
    getBoundingClientRect() { return this._rect; },
    contains() { return false; }
  };
}
const root = el({ tagName: "HTML" });
const hiddenGate = el({ tagName: "DIV", id: "gate", parentElement: root, style: { display: "none" } });
const all = [root, hiddenGate];
let parent = hiddenGate;
for (let i = 0; i < 60; i++) {
  const node = el({ tagName: "DIV", id: `d-${i}`, parentElement: parent, text: `hidden-${i}` });
  all.push(node);
  parent = node;
}
const deepBtn = el({ tagName: "BUTTON", id: "deep-btn", parentElement: parent });
all.push(deepBtn);

let styleCalls = 0;
globalThis.__mockWindow = { innerWidth: 1440, innerHeight: 900 };
globalThis.__mockGetComputedStyle = (node) => { styleCalls += 1; return node._style || {}; };
globalThis.__mockDocument = {
  documentElement: root,
  querySelectorAll(sel) {
    if (sel.includes("button")) return [deepBtn];
    if (sel === "img") return [];
    return all;
  }
};
globalThis.__getStyleCalls = () => styleCalls;
"#;

#[test]
fn dom_audit_caches_visibility_with_weakmap_on_deep_hidden_trees() {
    let script_literal = serde_json::to_string(DOM_AUDIT_SCRIPT).expect("serialize script");
    let harness = format!(
        r#"
const vm = require("node:vm");
{DEEP_HIDDEN_TREE_FIXTURE_JS}
const sandbox = {{
  window: globalThis.__mockWindow,
  document: globalThis.__mockDocument,
  getComputedStyle: globalThis.__mockGetComputedStyle,
  WeakMap, Set, Array, Math, String, Number, Boolean, Object, JSON,
}};
sandbox.globalThis = sandbox;
const result = vm.runInNewContext({script_literal}, sandbox);
process.stdout.write(JSON.stringify({{ result, styleCalls: globalThis.__getStyleCalls() }}));
"#
    );

    let output = Command::new("node")
        .arg("-e")
        .arg(harness)
        .output()
        .expect("node must execute deep hidden tree fixture");
    assert!(output.status.success(), "node failed: {}", String::from_utf8_lossy(&output.stderr));
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid JSON output");
    assert_eq!(parsed["result"]["passed"], true);
    let style_calls = parsed["styleCalls"].as_u64().expect("styleCalls number");
    // 63 nodes total; without WeakMap caching, 61 nodes * ~31 avg depth would be >1800 calls.
    assert!(
        style_calls <= 65,
        "expected O(N) style lookups <= 65 with WeakMap caching, got {style_calls}"
    );
}

#[test]
fn web_rs_has_zero_allow_attributes_and_playwright_supports_phases_and_cwd_require() {
    let web_rs = include_str!("../src/web.rs");
    assert!(!web_rs.contains("#[allow("), "web.rs must not contain any #[allow(...)] attributes");

    let dir = tempdir().expect("tempdir");
    for phase in ["before", "after"] {
        let script = build_playwright_runner_script("http://127.0.0.1:4173", phase, dir.path())
            .expect("build playwright script");
        assert!(script.contains(&format!(r#"const phase = "{phase}";"#)));
        assert!(script.contains(r#"createRequire(path.join(process.cwd(), "package.json"))"#));
        assert!(script.contains(r#"await import("playwright")"#));
    }

    let invalid = build_playwright_runner_script("http://127.0.0.1:4173", "../bad", dir.path());
    assert!(invalid.is_err(), "invalid phase must be rejected");
}
