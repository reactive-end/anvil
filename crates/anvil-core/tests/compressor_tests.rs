use anyhow::Result;
use tempfile::TempDir;

use anvil_core::compressor::{
    DEFAULT_MAX_LINES, DEFAULT_MAX_TOKENS, compress_output, select_parser, strip_ansi,
};

#[test]
fn tsc_pass_and_fail_compression() -> Result<()> {
    let pass = compress_output(None, "npx tsc --noEmit", 0, "", 50, 1200)?;
    assert!(pass.passed);
    assert!(pass.lines.is_empty());
    assert!(pass.summary.starts_with("PASS (tsc)"));

    let tmp = TempDir::new()?;
    let raw = "src/Button.tsx(14,7): error TS2322: Type 'number' is not assignable to type 'string'.\n";
    let fail = compress_output(Some(tmp.path()), "npx tsc --noEmit", 2, raw, 50, 1200)?;
    assert!(!fail.passed);
    assert_eq!(fail.tool, "tsc");
    assert_eq!(fail.total_errors, 1);
    assert_eq!(fail.lines, vec!["src/Button.tsx:14:7 [TS2322] Type 'number' is not assignable to type 'string'."]);
    assert!(fail.raw_log_path.as_ref().is_some_and(|p| p.exists()));
    Ok(())
}

#[test]
fn ansi_stripped_and_tsc_matches_token_boundaries_only() -> Result<()> {
    let colored = "\x1b[36msrc/Button.tsx\x1b[0m(\x1b[33m14\x1b[0m,\x1b[33m7\x1b[0m): \x1b[31merror TS2322\x1b[0m: Type mismatch\n";
    assert!(!strip_ansi(colored).contains('\x1b'));
    let res = compress_output(None, "./node_modules/.bin/tsc.cmd --noEmit", 2, colored, 50, 1200)?;
    assert_eq!(res.tool, "tsc");
    assert_eq!(res.total_errors, 1);
    assert_eq!(res.lines, vec!["src/Button.tsx:14:7 [TS2322] Type mismatch"]);

    assert_eq!(select_parser("npx vue-tsc --noEmit").name(), "tsc");
    assert_eq!(select_parser("ngc -p tsconfig.json").name(), "tsc");
    assert_eq!(select_parser("pytest -k test_postcreate").name(), "pytest/ruff");
    assert_eq!(select_parser("cargo test -p tsc-helper").name(), "cargo");
    assert_eq!(select_parser("cat tsc_notes.md").name(), "generic");
    Ok(())
}

#[test]
fn eslint_biome_vitest_jest_parsers() -> Result<()> {
    let eslint_raw = "src/app.ts\n  12:4  error  Unexpected any  @typescript-eslint/no-explicit-any\n";
    let eslint = compress_output(None, "npx eslint src/", 1, eslint_raw, 50, 1200)?;
    assert_eq!(eslint.tool, "eslint/biome");
    assert_eq!(eslint.total_errors, 1);
    assert!(eslint.lines[0].contains("src/app.ts:12:4"));

    let biome_raw = "src/main.ts:3:1 lint/suspicious/noExplicitAny\n";
    let biome = compress_output(None, "biome check .", 1, biome_raw, 50, 1200)?;
    assert_eq!(biome.tool, "eslint/biome");
    assert_eq!(biome.lines, vec!["src/main.ts:3:1 [lint/suspicious/noExplicitAny]"]);

    let vitest_raw = "FAIL src/cart.test.ts\n● cart › calculates total\nExpected: 10\nReceived: 12\n";
    let vitest = compress_output(None, "npx vitest run", 1, vitest_raw, 50, 1200)?;
    assert_eq!(vitest.tool, "vitest/jest");
    assert_eq!(vitest.total_errors, 2);

    let jest_pass = compress_output(None, "npx jest", 0, "Tests: 4 passed, 4 total", 50, 1200)?;
    assert!(jest_pass.passed);
    assert!(jest_pass.summary.contains("4 passed"));
    Ok(())
}

#[test]
fn pytest_ruff_gradle_dart_and_generic_parsers() -> Result<()> {
    let ruff_raw = "app/main.py:8:1: F401 `os` imported but unused\nFAILED tests/test_api.py::test_auth\n";
    let py = compress_output(None, "ruff check . && pytest", 1, ruff_raw, 50, 1200)?;
    assert_eq!(py.tool, "pytest/ruff");
    assert_eq!(py.total_errors, 2);

    let gradle_raw = "e: file:///app/Main.kt:21:9: Unresolved reference: foo\n> Task :app:compileDebugKotlin FAILED\n";
    let gradle = compress_output(None, "./gradlew :app:testDebugUnitTest", 1, gradle_raw, 50, 1200)?;
    assert_eq!(gradle.tool, "gradle/kotlin");
    assert_eq!(gradle.total_errors, 2);

    let dart_raw = "error • Undefined name 'bar' • lib/main.dart:5:3 • undefined_identifier\n";
    let dart = compress_output(None, "dart analyze", 1, dart_raw, 50, 1200)?;
    assert_eq!(dart.tool, "dart/flutter");
    assert_eq!(dart.lines, vec!["lib/main.dart:5:3 error [undefined_identifier] Undefined name 'bar'"]);

    let generic = compress_output(None, "make build", 2, "fatal error: missing header\n", 50, 1200)?;
    assert_eq!(generic.tool, "generic");
    assert_eq!(generic.total_errors, 1);
    Ok(())
}

#[test]
fn compressor_enforces_hard_line_and_token_caps() -> Result<()> {
    let mut big = String::new();
    for i in 1..=120 {
        big.push_str(&format!("src/f{i}.ts({i},1): error TS2304: Cannot find name 'x{i}'.\n"));
    }
    let out = compress_output(
        None,
        "tsc --noEmit",
        2,
        &big,
        DEFAULT_MAX_LINES,
        DEFAULT_MAX_TOKENS,
    )?;
    assert_eq!(out.total_errors, 120);
    assert!(out.truncated);
    assert!(out.lines.len() <= DEFAULT_MAX_LINES);
    let total_chars: usize = out.lines.iter().map(|l| l.len() + 1).sum();
    assert!(total_chars <= DEFAULT_MAX_TOKENS * 4);
    Ok(())
}

#[test]
fn cargo_parser_handles_check_clippy_test_and_rustc() -> Result<()> {
    assert_eq!(select_parser("cargo check --workspace").name(), "cargo");
    assert_eq!(select_parser("cargo clippy --all-targets -- -D warnings").name(), "cargo");
    assert_eq!(select_parser("rustc src/main.rs").name(), "cargo");

    let pass = compress_output(
        None,
        "cargo test -p anvil-core",
        0,
        "test result: ok. 12 passed; 0 failed; 0 ignored",
        50,
        1200,
    )?;
    assert!(pass.passed);
    assert!(pass.summary.contains("12 passed"));

    let raw_err = concat!(
        "error[E0308]: mismatched types\n",
        " --> src/lib.rs:42:9\n",
        "  |\n",
        "42|     \"abc\"\n",
        "  |     ^^^^^ expected `usize`, found `&str`\n",
        "warning: unused variable: `x`\n",
        " --> src/lib.rs:10:5\n",
        "test auth::tests::rejects_bad_token ... FAILED\n",
        "test result: FAILED. 3 passed; 1 failed; 0 ignored\n",
        "error: could not compile `demo` (lib) due to 1 previous error\n",
    );
    let fail = compress_output(None, "cargo test --workspace", 101, raw_err, 50, 1200)?;
    assert!(!fail.passed);
    assert_eq!(fail.tool, "cargo");
    assert!(fail.lines.iter().any(|l| l == "src/lib.rs:42:9 error[E0308]: mismatched types"));
    assert!(fail.lines.iter().any(|l| l == "src/lib.rs:10:5 warning: unused variable: `x`"));
    assert!(fail.lines.iter().any(|l| l == "test auth::tests::rejects_bad_token ... FAILED"));
    assert!(fail.lines.iter().any(|l| l.starts_with("test result: FAILED.")));
    Ok(())
}
