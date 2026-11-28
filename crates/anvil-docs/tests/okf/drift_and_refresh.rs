use std::fs;
use std::path::Path;

use anvil_docs::bundle::Bundle;
use anvil_docs::concept::{self, Concept};
use anvil_docs::create::{NewConcept, create_concept};
use anvil_docs::drift::{Drift, Freshness};
use anvil_docs::refresh::{refresh_concept, verify_concept};
use anvil_docs::trust::{Trust, trust_of};

use crate::common::{bundle, commit, repo, write};

fn documented(root: &Path) -> Bundle {
    let bundle = bundle(root);
    let spec = NewConcept {
        module: "orders".to_owned(),
        paths: vec!["src/orders/**".to_owned()],
        ..NewConcept::default()
    };
    create_concept(root, &bundle, &spec, None).expect("create");
    bundle
}

fn load(root: &Path, bundle: &Bundle) -> Concept {
    concept::scan(root, bundle, true).concepts.remove(0)
}

fn freshness(root: &Path, bundle: &Bundle) -> Freshness {
    Drift::new(root).check(&load(root, bundle))
}

#[test]
fn a_new_concept_is_fresh_and_ignores_unrelated_changes() {
    let dir = repo();
    let root = dir.path();
    let bundle = documented(root);
    assert_eq!(freshness(root, &bundle), Freshness::Fresh);
    write(root, "src/billing/invoice.ts", "export function bill() { return 1 }\n");
    assert_eq!(freshness(root, &bundle), Freshness::Fresh);
    commit(root, "billing change");
    assert_eq!(freshness(root, &bundle), Freshness::Fresh);
}

#[test]
fn committed_changes_make_a_concept_stale() {
    let dir = repo();
    let root = dir.path();
    let bundle = documented(root);
    write(root, "src/orders/service.ts", "export function createOrder() { return 2 }\n");
    commit(root, "orders change");
    let state = freshness(root, &bundle);
    assert!(state.reason().is_some_and(|r| r.contains("src/orders/service.ts")), "{state:?}");
}

#[test]
fn uncommitted_changes_make_a_concept_stale() {
    let dir = repo();
    let root = dir.path();
    let bundle = documented(root);
    write(root, "src/orders/service.ts", "export function createOrder() { return 3 }\n");
    assert!(freshness(root, &bundle).is_stale());
}

#[test]
fn untracked_files_make_a_concept_stale() {
    let dir = repo();
    let root = dir.path();
    let bundle = documented(root);
    write(root, "src/orders/refund.ts", "export function refund() {}\n");
    let state = freshness(root, &bundle);
    assert!(state.reason().is_some_and(|r| r.contains("refund.ts")), "{state:?}");
}

#[test]
fn an_unknown_synced_commit_is_stale() {
    let dir = repo();
    let root = dir.path();
    let bundle = documented(root);
    let mut concept = load(root, &bundle);
    concept.doc.set_str("x-anvil-synced-commit", "0123456789ab");
    let state = Drift::new(root).check(&concept);
    assert!(state.reason().is_some_and(|r| r.contains("unknown")), "{state:?}");
    concept.doc.remove_field("x-anvil-synced-commit");
    let state = Drift::new(root).check(&concept);
    assert!(state.reason().is_some_and(|r| r.contains("no x-anvil-synced-commit")), "{state:?}");
}

#[test]
fn refresh_resyncs_dirty_code_until_it_changes_again() {
    let dir = repo();
    let root = dir.path();
    let bundle = documented(root);
    write(root, "src/orders/service.ts", "export function createOrder() { return 4 }\n");
    assert!(freshness(root, &bundle).is_stale());
    refresh_concept(root, &bundle, &load(root, &bundle), None).expect("refresh");
    assert_eq!(freshness(root, &bundle), Freshness::Fresh);
    commit(root, "land the change");
    assert_eq!(freshness(root, &bundle), Freshness::Fresh);
    write(root, "src/orders/service.ts", "export function createOrder() { return 5 }\n");
    assert!(freshness(root, &bundle).is_stale());
    let log = fs::read_to_string(bundle.log_path()).expect("log");
    assert!(log.contains("Refreshed [shop/orders](orders/overview.md)."), "{log}");
}

#[test]
fn verify_records_a_human_review() {
    let dir = repo();
    let root = dir.path();
    let bundle = documented(root);
    assert_eq!(trust_of(&load(root, &bundle).doc), Trust::Unverified);
    let stamp = verify_concept(&bundle, &load(root, &bundle), "Ada").expect("verify");
    assert_eq!(stamp.by, "human:Ada");
    let reloaded = load(root, &bundle);
    assert_eq!(reloaded.doc.verified().len(), 1);
    assert_eq!(trust_of(&reloaded.doc), Trust::HumanReviewed);
    assert!(verify_concept(&bundle, &reloaded, "  ").is_err());
}
