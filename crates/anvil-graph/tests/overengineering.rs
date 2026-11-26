//! `check_overengineering`: new code that no non-test file uses is reported; everything else is silent.

use std::fs;
use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_core::git::AddedLines;
use anvil_graph::CodeGraphEngine;
use anvil_graph::impact::OverengineeringFinding;
use tempfile::TempDir;

fn write_repo_file(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(path, content).expect("write fixture file");
}

/// A TypeScript repository whose resolver demonstrably works: `app.ts` imports `base.ts`.
fn ts_repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    write_repo_file(dir.path(), "src/base.ts", "export function baseValue() { return 1; }\n");
    write_repo_file(
        dir.path(),
        "src/app.ts",
        "import { baseValue } from './base';\nexport const answer = baseValue();\n",
    );
    dir
}

fn unused_new_code(root: &Path, added: &[&str]) -> Vec<OverengineeringFinding> {
    let engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build graph");
    let added: Vec<String> = added.iter().map(|f| (*f).to_owned()).collect();
    engine.check_overengineering(&added, &AddedLines::default()).findings
}

#[test]
fn zero_consumers_is_an_error_naming_the_rule_and_the_file() {
    let repo = ts_repo();
    write_repo_file(repo.path(), "src/orphan.ts", "export function orphanTask() { return 3; }\n");
    let findings = unused_new_code(repo.path(), &["src/orphan.ts"]);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let finding = &findings[0];
    assert_eq!(finding.rule, "overengineering:unused-new-code");
    assert_eq!(
        (finding.file.as_str(), finding.line, finding.symbol.as_deref()),
        ("src/orphan.ts", 1, None)
    );
    assert!(
        finding.message.contains("no non-test consumer") && finding.message.contains("orphanTask")
    );
    assert!(finding.suggestion.contains("anvil-allow(overengineering): <reason>"));
}

#[test]
fn consumers_that_are_only_tests_do_not_count() {
    let repo = ts_repo();
    write_repo_file(
        repo.path(),
        "src/tested.ts",
        "export function onlyTestsUseMe() { return 4; }\n",
    );
    write_repo_file(
        repo.path(),
        "src/tested.test.ts",
        "import { onlyTestsUseMe } from './tested';\nexport const t = onlyTestsUseMe();\n",
    );
    assert_eq!(unused_new_code(repo.path(), &["src/tested.ts"]).len(), 1);
}

#[test]
fn a_whole_unused_file_is_reported_once_not_per_symbol() {
    let repo = ts_repo();
    write_repo_file(
        repo.path(),
        "src/trio.ts",
        "export function alpha() {}\nexport function beta() {}\nexport const GAMMA = 3;\n",
    );
    let findings = unused_new_code(repo.path(), &["src/trio.ts"]);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].symbol.is_none());
}

#[test]
fn an_unused_export_of_a_used_file_is_reported_by_name() {
    let repo = ts_repo();
    write_repo_file(
        repo.path(),
        "src/kit.ts",
        "export function usedHelper() { return 1; }\nexport function deadHelper() { return 2; }\n\
         export interface OnlyAType { id: number }\n",
    );
    write_repo_file(
        repo.path(),
        "src/screen.ts",
        "import { usedHelper } from './kit';\nimport type { OnlyAType } from './kit';\n\
         export const v: OnlyAType = { id: usedHelper() };\n",
    );
    let findings = unused_new_code(repo.path(), &["src/kit.ts"]);
    let symbols: Vec<_> = findings.iter().filter_map(|f| f.symbol.as_deref()).collect();
    assert_eq!(symbols, vec!["deadHelper"], "type-only use is a use: {findings:?}");
}

#[test]
fn entry_points_are_silent() {
    let repo = ts_repo();
    write_repo_file(repo.path(), "src/main.ts", "export function boot() {}\n");
    write_repo_file(
        repo.path(),
        "vite.config.ts",
        "export default { plugins: [] };\nexport const extra = 1;\n",
    );
    write_repo_file(
        repo.path(),
        "package.json",
        "{\"name\":\"x\",\"main\":\"src/published.ts\"}\n",
    );
    write_repo_file(repo.path(), "src/published.ts", "export function publicApi() {}\n");
    let findings =
        unused_new_code(repo.path(), &["src/main.ts", "vite.config.ts", "src/published.ts"]);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn allow_comments_silence_a_file_or_one_symbol() {
    let repo = ts_repo();
    write_repo_file(
        repo.path(),
        "src/kept.ts",
        "// anvil-allow(overengineering): consumed by the plugin loader at runtime\nexport function kept() {}\n",
    );
    write_repo_file(
        repo.path(),
        "src/partial.ts",
        "export function usedOne() {}\n// anvil-allow(overengineering): called by generated code\nexport function loaded() {}\n",
    );
    write_repo_file(
        repo.path(),
        "src/uses-partial.ts",
        "import { usedOne } from './partial';\nexport const z = usedOne();\n",
    );
    let findings = unused_new_code(repo.path(), &["src/kept.ts", "src/partial.ts"]);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn languages_without_resolved_references_are_silent() {
    let repo = ts_repo();
    write_repo_file(repo.path(), "tools/helpers.py", "def orphan_helper():\n    return 1\n");
    write_repo_file(repo.path(), "tools/other.py", "def another():\n    return 2\n");
    let findings = unused_new_code(repo.path(), &["tools/helpers.py"]);
    assert!(findings.is_empty(), "{findings:?}");
}

fn rust_repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write_repo_file(
        root,
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_repo_file(
        root,
        "src/lib.rs",
        "pub mod used;\npub mod dead;\npub mod reexported;\n\nuse crate::used::helper;\npub use crate::reexported::thing;\n\n\
         pub fn run() -> u32 {\n    helper()\n}\n",
    );
    write_repo_file(root, "src/used.rs", "pub fn helper() -> u32 {\n    1\n}\n");
    write_repo_file(root, "src/dead.rs", "pub fn never_called() -> u32 {\n    2\n}\n");
    write_repo_file(root, "src/reexported.rs", "pub fn thing() -> u32 {\n    3\n}\n");
    dir
}

/// A `mod foo;` declaration is not a use: a module nobody calls stays dead, a module that another file
/// imports or re-exports is alive.
#[test]
fn rust_modules_declared_by_their_parent_are_still_dead_when_nothing_uses_them() {
    let repo = rust_repo();
    let findings =
        unused_new_code(repo.path(), &["src/used.rs", "src/dead.rs", "src/reexported.rs"]);
    let files: Vec<&str> = findings.iter().map(|f| f.file.as_str()).collect();
    assert_eq!(files, vec!["src/dead.rs"], "{findings:?}");
}

#[test]
fn rust_entry_points_and_attributed_items_are_silent() {
    let repo = rust_repo();
    write_repo_file(repo.path(), "src/bin/tool.rs", "pub fn only_here() {}\n");
    write_repo_file(
        repo.path(),
        "src/ffi.rs",
        "#[no_mangle]\npub extern \"C\" fn exported_to_c() {}\n",
    );
    write_repo_file(
        repo.path(),
        "src/lib.rs",
        "pub mod used;\npub mod ffi;\n\nuse crate::used::helper;\n\npub fn run() -> u32 {\n    helper()\n}\n",
    );
    let findings = unused_new_code(repo.path(), &["src/bin/tool.rs", "src/ffi.rs", "src/lib.rs"]);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn a_helper_shared_by_other_files_tests_is_test_support_not_dead_code() {
    let repo = ts_repo();
    write_repo_file(repo.path(), "src/kit.ts", "export function mountForTest() { return 1; }\n");
    write_repo_file(
        repo.path(),
        "src/a.test.ts",
        "import { mountForTest } from './kit';\nexport const a = mountForTest();\n",
    );
    let findings = unused_new_code(repo.path(), &["src/kit.ts"]);
    assert!(findings.is_empty(), "{findings:?}");
}

/// `src/util.ts` is modified: line 2 predates the change (unused), lines 3 and 4 are new.
fn modified_util_repo() -> TempDir {
    let repo = ts_repo();
    write_repo_file(
        repo.path(),
        "src/util.ts",
        "export function usedOld() { return 1; }
export function deadOld() { return 2; }
         export function freshDead() { return 3; }
export function freshUsed() { return 4; }
",
    );
    write_repo_file(
        repo.path(),
        "src/user.ts",
        "import { usedOld, freshUsed } from './util';
export const sum = usedOld() + freshUsed();
",
    );
    repo
}

fn modified(root: &Path, file: &str, ranges: Vec<(u32, u32)>) -> Vec<OverengineeringFinding> {
    let engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build graph");
    let added = AddedLines::from_iter([(file.to_owned(), ranges)]);
    engine.check_overengineering(&[], &added).findings
}

#[test]
fn a_new_unused_function_in_a_modified_file_is_an_error() {
    let repo = modified_util_repo();
    let findings = modified(repo.path(), "src/util.ts", vec![(3, 4)]);
    let symbols: Vec<_> = findings.iter().filter_map(|f| f.symbol.as_deref()).collect();
    assert_eq!(symbols, vec!["freshDead"], "{findings:?}");
    assert_eq!((findings[0].rule, findings[0].line), ("overengineering:unused-new-code", 3));
}

#[test]
fn a_pre_existing_unused_function_is_never_reported() {
    let repo = modified_util_repo();
    let findings = modified(repo.path(), "src/util.ts", vec![(4, 4)]);
    assert!(findings.is_empty(), "deadOld and freshDead are not on added lines: {findings:?}");
}

#[test]
fn a_new_function_used_by_a_non_test_file_is_silent() {
    let repo = modified_util_repo();
    let findings = modified(repo.path(), "src/util.ts", vec![(4, 4)]);
    assert!(findings.is_empty(), "{findings:?}");
}
