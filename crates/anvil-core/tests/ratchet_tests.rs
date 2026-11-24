use std::fs;

use anyhow::Result;
use tempfile::TempDir;

use anvil_core::ratchet::{
    CandidateInput, CandidateOptions, PromotedRule, RuleCandidate, compile_promoted_rules,
    evaluate_candidate, evaluate_compiled, evaluate_promoted_rules, list_candidates, list_rules,
    load_promoted_rules, log_candidate, log_candidate_with_options, promote_candidate,
    test_candidate,
};
use anvil_core::traits::Severity;

fn sample_input(name: &str) -> CandidateInput {
    CandidateInput {
        rule_name: name.to_owned(),
        description: "Disallow raw console.log in production code".to_owned(),
        pattern: r"console\.log\(".to_owned(),
        languages: vec!["typescript".to_owned()],
        high_impact: false,
        deterministic: true,
        estimated_runtime_ms: 15,
        example: Some("src/api.ts:12".to_owned()),
    }
}

#[test]
fn ratchet_blocks_promotion_until_recurrence_and_fp_pass() -> Result<()> {
    let tmp = TempDir::new()?;
    let root = tmp.path();

    let c1 = log_candidate(root, sample_input("no-console-log"))?;
    assert_eq!(c1.recurrence, 1);
    let eval1 = evaluate_candidate(root, &c1.id, None)?;
    assert!(!eval1.eligible);
    assert!(!eval1.filter_recurrence_or_impact);
    assert!(promote_candidate(root, &c1.id).is_err());

    let mut second = sample_input("no-console-log");
    second.example = Some("src/worker.ts:44".to_owned());
    let c2 = log_candidate(root, second)?;
    assert_eq!((c2.id.as_str(), c2.recurrence), (c1.id.as_str(), 2));
    assert_eq!(list_candidates(root)?.len(), 1);

    let eval_fp = evaluate_candidate(root, &c2.id, Some(&["console.log('oops')"]))?;
    assert!(!eval_fp.eligible);
    assert!(!eval_fp.filter_zero_false_positives);
    Ok(())
}

#[test]
fn ratchet_promotes_once_triple_filter_passes() -> Result<()> {
    let tmp = TempDir::new()?;
    let root = tmp.path();

    let c1 = log_candidate(root, sample_input("no-console-log"))?;
    let c2 = log_candidate(root, sample_input("no-console-log"))?;
    assert_eq!(c1.id, c2.id);

    let eval_ok = evaluate_candidate(root, &c2.id, Some(&["logger.info('ok')", "const x = 1;"]))?;
    assert!(eval_ok.eligible);
    assert!(eval_ok.filter_recurrence_or_impact);
    assert!(eval_ok.filter_deterministic_fast);
    assert!(eval_ok.filter_zero_false_positives);

    let rule = promote_candidate(root, &c2.id)?;
    assert_eq!(rule.rule_name, "no-console-log");
    assert_eq!(rule.recurrence_at_promotion, 2);
    assert!(list_candidates(root)?.is_empty());
    assert_eq!(list_rules(root)?.len(), 1);
    Ok(())
}

#[test]
fn promoted_rules_evaluate_with_globs_and_anvil_allow_and_test_candidate() -> Result<()> {
    let tmp = TempDir::new()?;
    let root = tmp.path();

    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("node_modules/pkg"))?;
    fs::write(
        root.join("src/app.ts"),
        "console.log('hit1');\n// anvil-allow(no-console-log): debug CLI\nconsole.log('allowed');\n",
    )?;
    fs::write(root.join("node_modules/pkg/index.ts"), "console.log('ignored');\n")?;

    let c1 = log_candidate(root, sample_input("no-console-log"))?;
    let c2 = log_candidate(root, sample_input("no-console-log"))?;
    assert_eq!(c1.id, c2.id);

    let report = test_candidate(root, &c2.id)?;
    assert_eq!(report.files_scanned, 1);
    assert_eq!(report.match_count, 2);
    assert!(report.under_budget);

    evaluate_candidate(root, &c2.id, Some(&["logger.info('ok')"]))?;
    let mut rule = promote_candidate(root, &c2.id)?;
    rule.file_globs = vec!["src/**/*.ts".to_owned()];
    rule.severity = Some(Severity::Error);
    let rule_path = root.join(".harness/rules").join(format!("{}.json", rule.id));
    fs::write(&rule_path, serde_json::to_string_pretty(&rule)?)?;

    let loaded = load_promoted_rules(root)?;
    assert_eq!(loaded.len(), 1);
    let diags = evaluate_promoted_rules(&loaded, "src/app.ts", &fs::read_to_string(root.join("src/app.ts"))?);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].line, 1);
    assert_eq!(diags[0].severity, Severity::Error);
    assert!(evaluate_promoted_rules(&loaded, "other/app.ts", "console.log('x');").is_empty());

    let legacy_json = r#"{"id":"1","rule_name":"r","description":"d","languages":[],"recurrence":1,"high_impact":false,"deterministic":true,"estimated_runtime_ms":1,"fp_checked":false,"false_positives_found":0,"examples":[],"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}"#;
    let parsed_cand: RuleCandidate = serde_json::from_str(legacy_json)?;
    assert!(parsed_cand.pattern.is_none());
    assert!(parsed_cand.file_globs.is_empty());
    let legacy_rule = r#"{"id":"1","rule_name":"r","description":"d","languages":[],"recurrence_at_promotion":2,"promoted_at":"2026-01-01T00:00:00Z"}"#;
    let parsed_rule: PromotedRule = serde_json::from_str(legacy_rule)?;
    assert!(parsed_rule.pattern.is_none());
    Ok(())
}

#[test]
fn compiled_rules_and_comment_context_allow_and_options_work() -> Result<()> {
    let tmp = TempDir::new()?;
    let root = tmp.path();
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join(".harness/.state"))?;
    fs::write(root.join(".harness/.state/debug.ts"), "console.log('ignored');\n")?;

    let opts = CandidateOptions {
        file_globs: vec!["src/**/*.ts".to_owned()],
        severity: Some(Severity::Error),
        ..Default::default()
    };
    let c1 = log_candidate_with_options(root, sample_input("no-console-log"), opts.clone())?;
    let c2 = log_candidate_with_options(root, sample_input("no-console-log"), opts)?;
    assert_eq!(c1.id, c2.id);
    assert_eq!(c2.file_globs, vec!["src/**/*.ts"]);
    assert_eq!(c2.severity, Some(Severity::Error));

    evaluate_candidate(root, &c2.id, Some(&["logger.info('ok')"]))?;
    let rule = promote_candidate(root, &c2.id)?;
    let compiled = compile_promoted_rules(&[rule]);
    assert_eq!(compiled.len(), 1);

    let spoofed = "const s = \"anvil-allow(no-console-log): spoof\"; console.log('hit');\n";
    let diags_spoof = evaluate_compiled(&compiled, "src/a.ts", spoofed);
    assert_eq!(diags_spoof.len(), 1);

    let spoofed_comment_in_str = "const s = \"// anvil-allow(no-console-log): fake\";\nconsole.log('hit2');\n";
    let diags_spoof2 = evaluate_compiled(&compiled, "src/a.ts", spoofed_comment_in_str);
    assert_eq!(diags_spoof2.len(), 1);

    let allowed = "/* anvil-allow(no-console-log): real comment */\nconsole.log('ok');\n";
    assert!(evaluate_compiled(&compiled, "src/a.ts", allowed).is_empty());

    let py_allowed = "# anvil-allow(no-console-log): python-style comment\nconsole.log('ok');\n";
    assert!(evaluate_compiled(&compiled, "src/a.ts", py_allowed).is_empty());

    let sql_allowed = "-- anvil-allow(no-console-log): sql-style comment\nconsole.log('ok');\n";
    assert!(evaluate_compiled(&compiled, "src/a.ts", sql_allowed).is_empty());
    Ok(())
}

#[test]
fn ratchet_skips_comments_and_strings_and_supports_multiline_and_web_php_langs() -> Result<()> {
    let tmp = TempDir::new()?;
    let root = tmp.path();

    let c1 = log_candidate(root, sample_input("no-console-log"))?;
    log_candidate(root, sample_input("no-console-log"))?;
    evaluate_candidate(root, &c1.id, Some(&["logger.info('ok')"]))?;
    let mut rule = promote_candidate(root, &c1.id)?;
    rule.languages = vec!["vue".to_owned(), "svelte".to_owned(), "astro".to_owned()];
    let compiled = compile_promoted_rules(&[rule]);

    let mixed = concat!(
        "// console.log('in line comment')\n",
        "/* console.log('in block comment') */\n",
        "const s = \"console.log()\"; console.log('real');\n",
        "const t = `console.log('template')`;\n",
    );
    let diags = evaluate_compiled(&compiled, "src/App.vue", mixed);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].line, 3);
    assert_eq!(diags[0].column, 28);

    let mut ml_input = sample_input("no-empty-catch");
    ml_input.pattern = r"try\s*\{[\s\S]*?\}\s*catch\s*\([^)]*\)\s*\{\s*\}".to_owned();
    ml_input.languages = vec!["php".to_owned(), "blade".to_owned()];
    ml_input.high_impact = true;
    let ml_opts = CandidateOptions {
        multiline: true,
        ..Default::default()
    };
    let ml_cand = log_candidate_with_options(root, ml_input, ml_opts)?;
    assert!(ml_cand.multiline);
    evaluate_candidate(root, &ml_cand.id, Some(&["try { run(); } catch (e) { log(e); }"]))?;
    let ml_rule = promote_candidate(root, &ml_cand.id)?;
    let ml_compiled = compile_promoted_rules(&[ml_rule]);

    let php_src = "<?php\ntry {\n  doWork();\n} catch (Exception $e) {\n}\n";
    let ml_diags = evaluate_compiled(&ml_compiled, "resources/views/home.blade.php", php_src);
    assert_eq!(ml_diags.len(), 1);
    assert_eq!(ml_diags[0].line, 2);
    Ok(())
}

#[test]
fn test_candidate_sets_fp_checked_and_promote_supports_severity_override() -> Result<()> {
    let tmp = TempDir::new()?;
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(root.join("src/app.js"), "export const count = 1;\n")?;

    let mut input = sample_input("no-var-decl");
    input.pattern = r"\bvar\s+".to_owned();
    input.languages = vec!["js".to_owned()];
    input.high_impact = true;
    let cand = log_candidate(root, input)?;
    assert!(!cand.fp_checked);

    let report = anvil_core::ratchet::test_candidate(root, &cand.id)?;
    assert_eq!(report.files_scanned, 1);
    assert_eq!(report.match_count, 0);
    assert!(report.under_budget);

    // Must promote without calling `evaluate_candidate(..., Some(samples))` manually.
    let rule = anvil_core::ratchet::promote_candidate_with_severity(
        root,
        &cand.id,
        Some(anvil_core::traits::Severity::Error),
    )?;
    assert_eq!(rule.severity, Some(anvil_core::traits::Severity::Error));

    let compiled = compile_promoted_rules(&[rule]);
    let diags = evaluate_compiled(&compiled, "src/app.js", "var bad = 1;\n");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, anvil_core::traits::Severity::Error);
    Ok(())
}
