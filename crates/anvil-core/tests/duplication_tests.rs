use std::fs;
use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_core::duplication::check_added_duplication;
use anvil_core::git::{AddedLines, parse_added_lines};
use anvil_core::traits::Diagnostic;
use tempfile::TempDir;

const WINDOW: usize = 8;

fn logic(prefix: &str, count: usize) -> String {
    (0..count)
        .map(|i| format!("const {prefix}{i} = compute(step_{i}, offset_{i}) + scale_{i};"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
    fs::write(path, content).expect("write file");
}

fn whole_file(root: &Path, rel: &str) -> (String, Vec<(u32, u32)>) {
    let count = fs::read_to_string(root.join(rel)).expect("read").lines().count();
    (rel.to_owned(), vec![(1, u32::try_from(count).expect("lines"))])
}

/// Runs the check as if `new_files` were added in full and `repo_files` were tracked.
fn run(root: &Path, repo_files: &[&str], new_files: &[&str]) -> Vec<Diagnostic> {
    let repo: Vec<String> = repo_files.iter().map(|f| (*f).to_owned()).collect();
    let added: AddedLines = new_files.iter().map(|f| whole_file(root, f)).collect();
    check_added_duplication(root, &repo, &added, WINDOW)
}

fn existing_helper_repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let body = logic("total", 10);
    write(
        dir.path(),
        "src/util.ts",
        &format!("export function helper() {{\n{body}\n  return total0;\n}}\n"),
    );
    dir
}

#[test]
fn a_copy_of_an_unchanged_helper_is_an_error_naming_it() {
    let repo = existing_helper_repo();
    let body = logic("total", 10);
    write(
        repo.path(),
        "src/fresh.ts",
        &format!("export function again() {{\n{body}\n  return total1;\n}}\n"),
    );
    let diags = run(repo.path(), &["src/util.ts", "src/fresh.ts"], &["src/fresh.ts"]);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].rule, "duplication:new-code");
    assert_eq!((diags[0].file.as_str(), diags[0].line), ("src/fresh.ts", 2));
    assert!(diags[0].message.contains("`src/util.ts:2`"), "{}", diags[0].message);
}

#[test]
fn a_repeat_inside_new_code_is_reported_once_at_the_later_copy() {
    let dir = TempDir::new().expect("tempdir");
    let body = logic("part", 9);
    write(
        dir.path(),
        "src/new.ts",
        &format!("function a() {{\n{body}\n}}\nfunction b() {{\n{body}\n}}\n"),
    );
    let diags = run(dir.path(), &["src/new.ts"], &["src/new.ts"]);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].line, 13);
    assert!(diags[0].message.contains("`src/new.ts:2`"), "{}", diags[0].message);
}

#[test]
fn a_window_under_the_minimum_is_silent() {
    let repo = TempDir::new().expect("tempdir");
    let short = logic("short", WINDOW - 1);
    write(repo.path(), "src/a.ts", &short);
    write(repo.path(), "src/b.ts", &short);
    assert!(run(repo.path(), &["src/a.ts", "src/b.ts"], &["src/b.ts"]).is_empty());
}

#[test]
fn imports_and_trivial_statements_are_silent() {
    let repo = TempDir::new().expect("tempdir");
    let imports: String =
        (0..12).map(|i| format!("import {{ item{i} }} from './mod{i}';\n")).collect();
    let trivial = "return;\nbreak;\nOk(())\nelse {\n}\n".repeat(4);
    write(repo.path(), "src/a.ts", &format!("{imports}{trivial}"));
    write(repo.path(), "src/b.ts", &format!("{imports}{trivial}"));
    assert!(run(repo.path(), &["src/a.ts", "src/b.ts"], &["src/b.ts"]).is_empty());
}

#[test]
fn test_files_are_neither_checked_nor_a_source_of_duplicates() {
    let repo = existing_helper_repo();
    let body = logic("total", 10);
    write(repo.path(), "src/util.test.ts", &body);
    assert!(
        run(repo.path(), &["src/util.ts", "src/util.test.ts"], &["src/util.test.ts"]).is_empty()
    );
    write(repo.path(), "tests/copy.ts", &body);
    write(repo.path(), "src/fresh.ts", &body);
    let diags = run(repo.path(), &["tests/copy.ts", "src/fresh.ts"], &["src/fresh.ts"]);
    assert!(diags.is_empty(), "a test file is not a place to reuse code from: {diags:?}");
}

#[test]
fn allow_comments_silence_new_and_legacy_markers() {
    let repo = existing_helper_repo();
    let body = logic("total", 10);
    for (file, marker) in [
        ("src/new_marker.ts", "// anvil-allow(duplication): mirrors the vendored parser"),
        ("src/old_marker.ts", "// anvil-allow(sonar:duplication): mirrors the vendored parser"),
    ] {
        write(repo.path(), file, &format!("{marker}\n{body}\n"));
        assert!(run(repo.path(), &["src/util.ts", file], &[file]).is_empty(), "{file}");
    }
}

#[test]
fn only_windows_made_of_added_lines_are_reported() {
    let repo = existing_helper_repo();
    let body = logic("total", 10);
    write(repo.path(), "src/old_copy.ts", &body);
    let added: AddedLines = AddedLines::from_iter([("src/old_copy.ts".to_owned(), vec![(1, 3)])]);
    let repo_files = vec!["src/util.ts".to_owned(), "src/old_copy.ts".to_owned()];
    assert!(check_added_duplication(repo.path(), &repo_files, &added, WINDOW).is_empty());
}

#[test]
fn the_window_comes_from_the_top_level_setting_then_the_sonar_one() {
    let mut config = AnvilConfig::default();
    assert_eq!(config.duplication_window(), 8);
    config.sonar.duplication_min_lines = 10;
    assert_eq!(config.duplication_window(), 10);
    config.duplication_min_lines = Some(6);
    assert_eq!(config.duplication_window(), 6);
    config.duplication_min_lines = Some(1);
    assert_eq!(config.duplication_window(), 4);
}

#[test]
fn added_line_ranges_come_from_the_hunks() {
    let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,5 @@\n keep\n+one\n+two\n keep\n-gone\n+three\n \
                keep\n@@ -20,0 +23,1 @@\n+tail\n";
    let added = parse_added_lines(diff);
    assert_eq!(added["src/a.rs"], vec![(2, 3), (5, 5), (23, 23)]);
}
