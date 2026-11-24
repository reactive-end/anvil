//! Integration tests for Tier 0 deterministic AST rules.

use std::path::Path;

use anvil_ast::analyze_file;
use anvil_core::config::AnvilConfig;
use anvil_core::traits::Severity;

fn default_cfg() -> AnvilConfig {
    AnvilConfig::default()
}

#[test]
fn todo_in_string_literal_has_zero_diagnostics_while_comment_fails() {
    let cfg = default_cfg();
    let ts_clean = r#"
export const msg = "TODO: not a comment";
const label = 'FIXME HACK XXX inside literal';
"#;
    let clean = analyze_file(Path::new("src/clean.ts"), ts_clean, &cfg).expect("analyze clean");
    assert!(clean.diagnostics.is_empty(), "unexpected: {:?}", clean.diagnostics);

    let ts_bad = r#"
// TODO: fix later
export const x = 1;
"#;
    let bad = analyze_file(Path::new("src/bad.ts"), ts_bad, &cfg).expect("analyze bad");
    assert_eq!(bad.diagnostics.len(), 1);
    assert_eq!(bad.diagnostics[0].rule, "no-todo-comments");
    assert_eq!(bad.diagnostics[0].severity, Severity::Error);
    assert_eq!(bad.diagnostics[0].line, 2);

    let py_bad = "msg = \"TODO: ignore\"\n# FIXME: broken\n";
    let py = analyze_file(Path::new("src/app.py"), py_bad, &cfg).expect("analyze py");
    assert_eq!(py.diagnostics.len(), 1);
    assert_eq!(py.diagnostics[0].rule, "no-todo-comments");
}

#[test]
fn narrative_comments_warn_while_concise_jsdoc_passes() {
    let cfg = default_cfg();
    let concise = r#"
/**
 * Computes normalized invoice total in cents.
 * @param cents Raw amount
 */
export function normalize(cents: number): number {
    // Preserve integer precision across currencies.
    return Math.round(cents);
}
"#;
    let ok = analyze_file(Path::new("src/money.ts"), concise, &cfg).expect("analyze concise");
    assert!(ok.diagnostics.is_empty(), "unexpected: {:?}", ok.diagnostics);

    let narrative = r#"
/*
 * Changelog:
 * Updated by Alice after previously this caused an issue.
 */
export const y = 2;
"#;
    let warned = analyze_file(Path::new("src/old.ts"), narrative, &cfg).expect("analyze narrative");
    assert_eq!(warned.diagnostics.len(), 1);
    assert_eq!(warned.diagnostics[0].rule, "no-narrative-comments");
    assert_eq!(warned.diagnostics[0].severity, Severity::Warning);
}

#[test]
fn escape_hatches_blocked_unless_structured_anvil_allow() {
    let cfg = default_cfg();

    let ts_fail = "// @ts-ignore\n// eslint-disable-next-line\nexport const a = 1;\n";
    let res = analyze_file(Path::new("src/escapes.ts"), ts_fail, &cfg).expect("ts escapes");
    assert_eq!(res.diagnostics.len(), 2);
    assert!(res.diagnostics.iter().all(|d| d.rule == "no-escape-hatches"));

    let ts_unstructured = "// anvil-allow(ts-ignore): missing ref\n// @ts-ignore\nexport const a = 1;\n";
    let res_unstructured = analyze_file(Path::new("src/u.ts"), ts_unstructured, &cfg).expect("u");
    assert_eq!(res_unstructured.diagnostics.len(), 1);

    let ts_allowed = "// anvil-allow(ts-ignore): vendor sdk lacks dts [PROJ-42]\n// @ts-ignore\nexport const a = 1;\n";
    let res_ok = analyze_file(Path::new("src/ok.ts"), ts_allowed, &cfg).expect("ts allowed");
    assert!(res_ok.diagnostics.is_empty(), "unexpected: {:?}", res_ok.diagnostics);

    let css_fail = ".btn { color: red !important; }\n";
    let css = analyze_file(Path::new("src/btn.css"), css_fail, &cfg).expect("css fail");
    assert_eq!(css.diagnostics.len(), 1);
    assert_eq!(css.diagnostics[0].rule, "no-escape-hatches");

    let css_ok = "/* anvil-allow(important): third-party modal override [UI-9] */\n.btn { color: red !important; }\n";
    let css_pass = analyze_file(Path::new("src/btn.css"), css_ok, &cfg).expect("css pass");
    assert!(css_pass.diagnostics.is_empty());

    let py_fail = "x = eval('1')  # noqa: S307\n";
    let py = analyze_file(Path::new("src/eval.py"), py_fail, &cfg).expect("py fail");
    assert_eq!(py.diagnostics.len(), 1);

    let kt_fail = "@Suppress(\"UNCHECKED_CAST\")\nfun cast(x: Any) = x as List<String>\n";
    let kt = analyze_file(Path::new("src/Cast.kt"), kt_fail, &cfg).expect("kt fail");
    assert_eq!(kt.diagnostics.len(), 1);

    let dart_fail = "// ignore: unused_local_variable\nvoid f() { final x = 1; }\n";
    let dart = analyze_file(Path::new("lib/f.dart"), dart_fail, &cfg).expect("dart fail");
    assert_eq!(dart.diagnostics.len(), 1);

    let scss_src = "// TODO: scss todo\n.box { margin: 0 !important; }\n";
    let scss = analyze_file(Path::new("src/box.scss"), scss_src, &cfg).expect("scss");
    assert_eq!(scss.diagnostics.len(), 2, "scss: {:?}", scss.diagnostics);
}

#[test]
fn barrel_rule_flags_pure_reexports_and_allows_real_logic() {
    let cfg = default_cfg();
    let pure_barrel = "export { Button } from './button';\nexport * from './card';\n";
    let res = analyze_file(Path::new("src/index.ts"), pure_barrel, &cfg).expect("barrel");
    assert_eq!(res.diagnostics.len(), 1);
    assert_eq!(res.diagnostics[0].rule, "no-barrel-files");

    let with_logic = "export { Button } from './button';\nexport function bootstrap() { return 1; }\n";
    let ok = analyze_file(Path::new("src/index.ts"), with_logic, &cfg).expect("with logic");
    assert!(ok.diagnostics.is_empty(), "unexpected: {:?}", ok.diagnostics);

    let non_index = "export { Button } from './button';\n";
    let ok2 = analyze_file(Path::new("src/public_api.ts"), non_index, &cfg).expect("non index");
    assert!(ok2.diagnostics.is_empty());

    let header_allowed = concat!(
        "#!/usr/bin/env node\n",
        "// Copyright (c) 2026 Anvil\n",
        "// anvil-allow(no-barrel-files): public package entrypoint [ANV-20]\n",
        "export { Button } from './button';\n",
    );
    let ok3 = analyze_file(Path::new("src/index.ts"), header_allowed, &cfg).expect("line 3 allow");
    assert!(ok3.diagnostics.is_empty(), "unexpected: {:?}", ok3.diagnostics);
}

#[test]
fn strict_types_detects_explicit_any_and_dynamic_without_identifier_false_positives() {
    let cfg = default_cfg();
    let ts_src = r#"
export const company = "any";
export const anyValue = 42;
export function parse(input: any): unknown {
    return input as any;
}
"#;
    let ts = analyze_file(Path::new("src/types.ts"), ts_src, &cfg).expect("ts strict");
    assert_eq!(ts.diagnostics.len(), 2, "diagnostics: {:?}", ts.diagnostics);
    assert!(ts.diagnostics.iter().all(|d| d.rule == "no-explicit-any"));
    assert_eq!(ts.diagnostics[0].line, 4);
    assert_eq!(ts.diagnostics[1].line, 5);

    let dart_src = r#"
const label = "dynamic";
void handle(dynamic payload) {}
"#;
    let dart = analyze_file(Path::new("lib/api.dart"), dart_src, &cfg).expect("dart strict");
    assert_eq!(dart.diagnostics.len(), 1);
    assert_eq!(dart.diagnostics[0].rule, "no-unnecessary-dynamic");
}

#[test]
fn root_hygiene_checks_relative_root_files_against_allowlist() {
    let cfg = default_cfg();
    let bad = analyze_file(Path::new("scratch_notes.ts"), "export const x = 1;", &cfg)
        .expect("root scratch");
    assert_eq!(bad.diagnostics.len(), 1);
    assert_eq!(bad.diagnostics[0].rule, "root-hygiene");

    let allowed = analyze_file(Path::new("tasks.json"), "{}", &cfg).expect("root allowlist");
    assert!(allowed.diagnostics.is_empty());

    let nested = analyze_file(Path::new("src/scratch_notes.ts"), "export const x = 1;", &cfg)
        .expect("nested");
    assert!(nested.diagnostics.is_empty());
}

#[test]
fn kotlin_unverified_permission_callback_is_flagged_while_handled_passes() {
    let cfg = default_cfg();
    let bad_kt = r#"
class MainActivity {
    val launcher = registerForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { /* assumed granted */ }
}
"#;
    let bad = analyze_file(Path::new("app/MainActivity.kt"), bad_kt, &cfg).expect("parse bad kt");
    assert!(
        bad.diagnostics.iter().any(|d| d.rule == "no-unverified-permission"),
        "expected no-unverified-permission diagnostic, got: {:?}",
        bad.diagnostics
    );

    let good_kt = r#"
class MainActivity {
    val launcher = registerForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { granted ->
        if (granted) {
            notifyReady()
        } else {
            openSettings()
        }
    }
}
"#;
    let good = analyze_file(Path::new("app/MainActivity.kt"), good_kt, &cfg).expect("parse good kt");
    assert!(
        !good.diagnostics.iter().any(|d| d.rule == "no-unverified-permission"),
        "expected 0 permission diagnostics, got: {:?}",
        good.diagnostics
    );

    let unchecked_ts = "Notification.requestPermission();\nnavigator.permissions.query({ name: 'notifications' }).then(() => {});\n";
    let bad_ts = analyze_file(Path::new("src/perm.ts"), unchecked_ts, &cfg).expect("parse bad ts");
    assert_eq!(
        bad_ts.diagnostics.iter().filter(|d| d.rule == "no-unverified-permission").count(),
        2,
        "expected 2 JS/TS permission diagnostics, got: {:?}",
        bad_ts.diagnostics
    );

    let checked_ts = "const st = await Notification.requestPermission();\nif (st === 'granted') { initPush(); }\n";
    let ok_ts = analyze_file(Path::new("src/perm_ok.ts"), checked_ts, &cfg).expect("parse ok ts");
    assert!(
        !ok_ts.diagnostics.iter().any(|d| d.rule == "no-unverified-permission"),
        "expected 0 JS/TS permission diagnostics, got: {:?}",
        ok_ts.diagnostics
    );
}

#[test]
fn permission_substring_traps_and_rust_inner_docs_behave_accurately() {
    let cfg = default_cfg();
    let sneaky_kt = r#"
class SubstringTrap {
    val l1 = registerForActivityResult(ActivityResultContracts.RequestPermission()) { notifyUser() }
    val l2 = registerForActivityResult(ActivityResultContracts.RequestPermission()) { val s = "ok!" }
}
"#;
    let sneaky = analyze_file(Path::new("app/Trap.kt"), sneaky_kt, &cfg).expect("sneaky kt");
    assert_eq!(
        sneaky.diagnostics.iter().filter(|d| d.rule == "no-unverified-permission").count(),
        2
    );

    let forwarded_kt = r#"
class Forwarded {
    val l = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        handleResult(granted)
    }
}
"#;
    let forwarded = analyze_file(Path::new("app/Fwd.kt"), forwarded_kt, &cfg).expect("fwd kt");
    assert!(!forwarded.diagnostics.iter().any(|d| d.rule == "no-unverified-permission"));

    let module_doc = (0..8)
        .map(|i| format!("//! Crate overview line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let doc_res = analyze_file(Path::new("src/lib.rs"), &module_doc, &cfg).expect("rust doc");
    assert!(
        !doc_res.diagnostics.iter().any(|d| d.rule == "no-narrative-comments"),
        "Rust `//!` doc comment must not trigger no-narrative-comments: {:?}",
        doc_res.diagnostics
    );
}
