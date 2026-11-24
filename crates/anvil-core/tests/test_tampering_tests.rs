use anvil_core::test_tampering::{check_test_tampering_diff, is_test_file};

#[test]
fn recognizes_test_paths_and_detects_skip_and_dropped_assertions() {
    assert!(is_test_file("crates/anvil-core/tests/foo_tests.rs"));
    assert!(is_test_file("src/auth/login.spec.ts"));
    assert!(!is_test_file("src/auth/login.ts"));

    let skip_marker = concat!("it", ".skip(");
    let diff = format!(
        "@@ -1,4 +1,2 @@\n-assert_eq!(a, 1);\n-assert_eq!(b, 2);\n+{skip_marker}'disabled', () => {{}});\n"
    );
    let diags = check_test_tampering_diff("src/auth.spec.ts", &diff);
    assert_eq!(diags.len(), 2);
    assert!(diags.iter().all(|d| d.rule == "no-test-tampering"));

    let bare_allow = "+// anvil-allow(no-test-tampering)\n-assert_eq!(a, 1);\n-assert_eq!(b, 2);\n";
    assert!(
        !check_test_tampering_diff("src/auth.spec.ts", bare_allow).is_empty(),
        "bare anvil-allow without `: <reason>` must NOT bypass test tampering guard"
    );

    let allowed = "+// anvil-allow(no-test-tampering): replaced by e2e suite\n-assert_eq!(a, 1);\n-assert_eq!(b, 2);\n";
    assert!(check_test_tampering_diff("src/auth.spec.ts", allowed).is_empty());
}
