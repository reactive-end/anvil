use std::fs;
use std::path::Path;

use anvil_docs::repo_files;
use anvil_docs::validate::{Issue, Severity, validate_bundle};

use crate::common::{bundle, index_text, repo, write};

fn concept(extra: &str, body: &str) -> String {
    format!("---\ntype: Code Module\n{extra}---\n{body}")
}

fn run(root: &Path) -> Vec<Issue> {
    validate_bundle(root, &bundle(root), &repo_files::list(root))
}

fn has(issues: &[Issue], needle: &str) -> bool {
    issues.iter().any(|i| i.severity == Severity::Error && i.message.contains(needle))
}

#[test]
fn a_valid_bundle_has_no_findings() {
    let dir = repo();
    let root = dir.path();
    write(root, ".harness/docs/shop/index.md", &index_text());
    write(root, ".harness/docs/shop/log.md", "# Log\n");
    let extra = "x-anvil-paths:\n  - src/orders\n  - src/orders/*.ts\nstatus: draft\n";
    let body = "# Orders\n\n[home](/index.md) [web](https://example.com) [up](#top)\n";
    write(root, ".harness/docs/shop/orders/overview.md", &concept(extra, body));
    write(root, ".harness/docs/shop/README.md", "no frontmatter, not a concept\n");
    assert_eq!(run(root), Vec::<Issue>::new());
}

#[test]
fn frontmatter_without_type_is_an_error() {
    let dir = repo();
    let root = dir.path();
    write(root, ".harness/docs/shop/index.md", &index_text());
    write(root, ".harness/docs/shop/orders/overview.md", "---\ntitle: Orders\n---\n# Orders\n");
    assert!(has(&run(root), "missing required `type`"));
}

#[test]
fn reserved_names_cannot_be_concepts() {
    let dir = repo();
    let root = dir.path();
    write(root, ".harness/docs/shop/index.md", &index_text());
    write(root, ".harness/docs/shop/log.md", "---\ntype: Log\n---\n# Log\n");
    write(root, ".harness/docs/shop/orders/index.md", "---\ntype: Code Module\n---\n");
    let issues = run(root);
    assert!(has(&issues, "`log.md` is reserved"), "{issues:?}");
    assert!(has(&issues, "`index.md` is reserved"), "{issues:?}");
}

#[test]
fn root_index_must_declare_okf_version() {
    let dir = repo();
    let root = dir.path();
    write(root, ".harness/docs/shop/index.md", "---\ntitle: Shop\n---\n# shop\n");
    assert!(has(&run(root), "okf_version"));
    write(root, ".harness/docs/shop/index.md", "# shop\n");
    assert!(has(&run(root), "okf_version"));
    fs::remove_file(root.join(".harness/docs/shop/index.md")).expect("remove");
    assert!(has(&run(root), "no index.md"));
}

#[test]
fn broken_links_are_errors_and_external_ones_are_not_read() {
    let dir = repo();
    let root = dir.path();
    write(root, ".harness/docs/shop/index.md", &index_text());
    // Links that leave the bundle point at real repo files and at nothing at all; neither is read.
    let body = "[gone](missing.md) [ok](../index.md) [out](../../../../src/orders/service.ts) [far](../../../../../../../nowhere.md) [mail](mailto:a@b.c)\n\n```\n[fenced](also-missing.md)\n```\n`[inline](also-missing.md)`\n";
    write(root, ".harness/docs/shop/orders/overview.md", &concept("", body));
    let issues = run(root);
    let messages: Vec<&str> = issues.iter().map(|i| i.message.as_str()).collect();
    assert_eq!(messages, ["broken link `missing.md`"], "{issues:?}");
}

#[test]
fn globs_that_match_nothing_are_errors() {
    let dir = repo();
    let root = dir.path();
    write(root, ".harness/docs/shop/index.md", &index_text());
    let extra = "x-anvil-paths:\n  - src/ghost/**\n  - src/orders/**\n";
    write(root, ".harness/docs/shop/ghost/overview.md", &concept(extra, "# Ghost\n"));
    let issues = run(root);
    assert!(has(&issues, "glob `src/ghost/**` matches no file"), "{issues:?}");
    assert!(!has(&issues, "src/orders/**"), "{issues:?}");
}

#[test]
fn field_shapes_are_checked() {
    let dir = repo();
    let root = dir.path();
    write(root, ".harness/docs/shop/index.md", &index_text());
    let extra = "status: wip\ngenerated: yes\nstale_after: someday\nx-anvil-paths: src\n";
    write(root, ".harness/docs/shop/odd/overview.md", &concept(extra, "# Odd\n"));
    let issues = run(root);
    assert!(has(&issues, "`status` must be one of"));
    assert!(has(&issues, "`generated` needs"));
    assert!(has(&issues, "`stale_after`"));
    assert!(has(&issues, "list of glob strings"));
}
