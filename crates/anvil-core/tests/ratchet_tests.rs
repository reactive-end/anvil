use anyhow::Result;
use tempfile::TempDir;

use anvil_core::ratchet::{
    CandidateInput, evaluate_candidate, list_candidates, list_rules, log_candidate,
    promote_candidate,
};

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
