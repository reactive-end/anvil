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

#[test]
fn catches_rust_ignore_avoids_iter_skip_fp_and_supports_py_dart_gd_php() {
    assert!(is_test_file("test/widget_test.dart"));
    assert!(is_test_file("tests/player_test.gd"));

    let rust_ignore = "@@ -1,2 +1,3 @@\n #[test]\n+#[ignore = \"slow\"]\n fn slow_case() {}\n";
    let ignore_diags = check_test_tampering_diff("crates/demo/tests/unit_test.rs", rust_ignore);
    assert_eq!(ignore_diags.len(), 1);
    assert!(ignore_diags[0].message.contains("#[ignore"));

    let quoted_fixture = concat!(
        "@@ -40,1 +40,3 @@\n let a = 1;\n",
        "+    let fixture = \"+#[ignore = \\\"slow\\\"]\";\n",
        "+    let js = 'it.skip(';\n",
    );
    assert!(
        check_test_tampering_diff("crates/demo/tests/unit_test.rs", quoted_fixture).is_empty(),
        "markers quoted inside string literals are fixtures, not skipped tests"
    );
    let after_literal =
        "@@ -40,1 +40,3 @@\n let a = 1;\n+const s = 'x';\n+it.skip('later', () => {});\n";
    let located = check_test_tampering_diff("src/a.spec.ts", after_literal);
    assert_eq!(located.len(), 1);
    assert_eq!(located[0].line, 42, "diagnostics must point at the offending line");

    let iter_skip = "@@ -1,2 +1,2 @@\n-let items: Vec<_> = src.iter().collect();\n+let items: Vec<_> = src.iter().skip(1).collect();\n";
    assert!(
        check_test_tampering_diff("crates/demo/tests/unit_test.rs", iter_skip).is_empty(),
        "iter().skip(1) must not false-positive"
    );

    let py_removed = "@@ -1,3 +1,1 @@\n-    assert a == 1\n-    assert b == 2\n";
    assert_eq!(check_test_tampering_diff("tests/test_math.py", py_removed).len(), 1);

    let php_removed = "@@ -1,3 +1,1 @@\n-$this->assertSame(1, $a);\n-$this->assertCount(2, $b);\n";
    assert_eq!(check_test_tampering_diff("tests/OrderTest.php", php_removed).len(), 1);
}
