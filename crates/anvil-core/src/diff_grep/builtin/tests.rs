//! Engine tests: ignore files, hidden policy, caps, context, matcher flags, globs, ordering.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::config::search::SearchBackend;
use crate::diff_grep::{GrepOptions, GrepReport, grep_with_backend, render};

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, content).expect("write");
}

fn search(root: &Path, opts: &GrepOptions) -> GrepReport {
    grep_with_backend(root, opts, SearchBackend::Builtin).expect("grep")
}

fn paths(report: &GrepReport) -> Vec<&str> {
    report.files.iter().map(|f| f.path.as_str()).collect()
}

#[test]
fn nested_gitignore_negation_and_dot_ignore_are_honoured_without_a_git_repo() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    write(root, ".gitignore", "*.log\n!keep.log\n");
    write(root, ".ignore", "skipme.txt\n");
    write(root, "pkg/.gitignore", "secret.txt\n");
    for rel in ["debug.log", "keep.log", "skipme.txt", "pkg/secret.txt", "pkg/open.txt", "a.txt"] {
        write(root, rel, "the needle\n");
    }

    let report = search(root, &GrepOptions::new("needle"));
    assert_eq!(paths(&report), ["a.txt", "keep.log", "pkg/open.txt"]);
    assert_eq!(report.total_matches, 3);
}

#[test]
fn hidden_entries_are_skipped_unless_requested_but_ci_and_harness_stay_searchable() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    write(root, ".secret/notes.txt", "needle\n");
    write(root, ".dotfile", "needle\n");
    write(root, ".github/workflows/ci.yml", "needle\n");
    write(root, ".harness/rules/r.md", "needle\n");
    write(root, ".harness/docs/d.md", "needle\n");
    write(root, ".harness/skills/s/SKILL.md", "needle\n");
    write(root, ".harness/config.json", "needle\n");
    write(root, ".harness/tasks/T-1/progress.md", "needle\n");
    write(root, ".harness/memory/chunks/c.json", "needle\n");
    write(root, ".harness/candidates/c.json", "needle\n");
    write(root, ".harness/.state/cache.txt", "needle\n");
    write(root, ".harness/.worktrees/w/x.txt", "needle\n");
    write(root, ".git/config", "needle\n");
    write(root, "visible.txt", "needle\n");

    let default = search(root, &GrepOptions::new("needle"));
    let readable = [
        ".github/workflows/ci.yml",
        ".harness/config.json",
        ".harness/docs/d.md",
        ".harness/rules/r.md",
        ".harness/skills/s/SKILL.md",
    ];
    let mut expected: Vec<&str> = readable.to_vec();
    expected.push("visible.txt");
    assert_eq!(paths(&default), expected, "ledgers under .harness/ are never searched");

    let mut opts = GrepOptions::new("needle");
    opts.hidden = true;
    let hidden = search(root, &opts);
    let mut expected_hidden = vec![".dotfile"];
    expected_hidden.extend(readable);
    expected_hidden.extend([".secret/notes.txt", "visible.txt"]);
    assert_eq!(
        paths(&hidden),
        expected_hidden,
        "--hidden never reaches .git, the .harness ledgers, .harness/.state or worktrees"
    );
}

#[test]
fn binary_and_oversized_files_are_skipped_but_text_with_late_matches_is_not() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    write(root, "ok.txt", "needle\n");
    fs::write(root.join("blob.bin"), b"needle\x00binary").expect("bin");
    let mut huge = vec![b'a'; 520 * 1024];
    huge.extend_from_slice(b"\nneedle\n");
    fs::write(root.join("huge.txt"), huge).expect("huge");

    let report = search(root, &GrepOptions::new("needle"));
    assert_eq!(paths(&report), ["ok.txt"]);
    assert_eq!(report.files_searched, 1, "binary and oversized files are not counted as searched");
}

#[test]
fn total_matches_stay_exact_when_output_is_capped() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    for idx in 0..30 {
        write(root, &format!("src/f{idx:02}.txt"), "hit\nhit\nmiss\nhit\n");
    }
    let mut opts = GrepOptions::new("hit");
    opts.max_matches = 7;
    opts.max_per_file = 2;
    let report = search(root, &opts);

    assert_eq!(report.total_matches, 90);
    assert_eq!(report.files_with_matches, 30);
    assert!(report.truncated);
    let shown: usize = report.files.iter().map(|f| f.matches.len()).sum();
    assert_eq!(shown, 7);
    assert_eq!(report.files[0].path, "src/f00.txt");
    assert_eq!((report.files[0].matches.len(), report.files[0].total), (2, 3));
    assert!(render(&report, 1500).contains("(+1 more in this file)"));

    let mut tiny = GrepOptions::new("hit");
    tiny.max_tokens = 10;
    let budgeted = search(root, &tiny);
    assert_eq!(budgeted.total_matches, 90);
    assert!(
        budgeted.truncated && budgeted.files.iter().map(|f| f.matches.len()).sum::<usize>() < 10
    );
}

#[test]
fn context_lines_are_attached_and_rendered_with_dash_markers() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    let body: String = (1..=20)
        .map(|n| match n {
            5 | 6 | 15 => format!(
                "needle {n}
"
            ),
            _ => format!(
                "line {n}
"
            ),
        })
        .collect();
    write(root, "a.txt", &body);

    let mut opts = GrepOptions::new("needle");
    opts.context = 2;
    let report = search(root, &opts);
    let matches = &report.files[0].matches;
    assert_eq!(matches.len(), 3);
    assert_eq!(matches[0].before, [(3, "line 3".to_owned()), (4, "line 4".to_owned())]);
    assert!(
        matches[0].after.is_empty(),
        "line 6 is itself a match, so it is not context of line 5"
    );
    assert_eq!(matches[1].after, [(7, "line 7".to_owned()), (8, "line 8".to_owned())]);
    assert_eq!(matches[2].before, [(13, "line 13".to_owned()), (14, "line 14".to_owned())]);
    assert_eq!(matches[2].after.len(), 2);

    let text = render(&report, 1500);
    assert!(
        text.contains(
            "  4- line 4
  5: needle 5
  6: needle 6
  7- line 7"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "  8- line 8
  --
  13- line 13"
        ),
        "gap marker between groups: {text}"
    );
}

#[test]
fn fixed_word_and_case_flags() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    write(root, "a.txt", "a.b\naxb\nconcatenate\ncat\nFoo\nfoo\nFOO\n");

    let count = |configure: &dyn Fn(&mut GrepOptions)| {
        let mut opts = GrepOptions::new("x");
        configure(&mut opts);
        search(root, &opts).total_matches
    };
    assert_eq!(count(&|o| o.pattern = "a.b".into()), 2, "regex dot matches both");
    assert_eq!(
        count(&|o| {
            o.pattern = "a.b".into();
            o.fixed_strings = true
        }),
        1
    );
    assert_eq!(count(&|o| o.pattern = "cat".into()), 2);
    assert_eq!(
        count(&|o| {
            o.pattern = "cat".into();
            o.word = true
        }),
        1
    );
    assert_eq!(
        count(&|o| o.pattern = "foo".into()),
        3,
        "smart case: lowercase pattern is insensitive"
    );
    assert_eq!(
        count(&|o| o.pattern = "Foo".into()),
        1,
        "smart case: uppercase pattern is sensitive"
    );
    assert_eq!(
        count(&|o| {
            o.pattern = "Foo".into();
            o.ignore_case = true
        }),
        3
    );
    assert_eq!(
        count(&|o| {
            o.pattern = "foo".into();
            o.smart_case = false
        }),
        1
    );
}

#[test]
fn globs_paths_and_invalid_inputs() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    write(root, "src/a.rs", "needle\n");
    write(root, "src/b.md", "needle\n");
    write(root, "docs/c.md", "needle\n");
    write(root, "top.rs", "needle\n");

    let with = |configure: &dyn Fn(&mut GrepOptions)| {
        let mut opts = GrepOptions::new("needle");
        configure(&mut opts);
        search(root, &opts)
    };
    assert_eq!(paths(&with(&|o| o.globs = vec!["*.rs".into()])), ["src/a.rs", "top.rs"]);
    assert_eq!(paths(&with(&|o| o.globs = vec!["!*.md".into()])), ["src/a.rs", "top.rs"]);
    assert_eq!(paths(&with(&|o| o.paths = vec!["src".into()])), ["src/a.rs", "src/b.md"]);
    assert_eq!(
        paths(&with(&|o| o.paths = vec!["top.rs".into(), "docs".into()])),
        ["docs/c.md", "top.rs"]
    );
    assert_eq!(
        paths(&with(&|o| {
            o.paths = vec!["src".into()];
            o.globs = vec!["*.md".into()]
        })),
        ["src/b.md"]
    );

    let mut missing = GrepOptions::new("needle");
    missing.paths = vec!["nope".into()];
    assert!(grep_with_backend(root, &missing, SearchBackend::Builtin).is_err());
    let mut outside = GrepOptions::new("needle");
    outside.paths = vec!["..".into()];
    assert!(grep_with_backend(root, &outside, SearchBackend::Builtin).is_err());
    assert!(grep_with_backend(root, &GrepOptions::new("("), SearchBackend::Builtin).is_err());
}

#[test]
fn order_is_deterministic_and_sorted_across_threads() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    for idx in (0..120).rev() {
        write(root, &format!("d{}/file{idx:03}.txt", idx % 7), &format!("needle {idx}\n"));
    }
    write(root, "a.txt", "needle\n");
    write(root, "a/b.txt", "needle\n");

    let first = search(root, &GrepOptions::new("needle"));
    let second = search(root, &GrepOptions::new("needle"));
    assert_eq!(first.files, second.files);
    let listed: Vec<String> = first.files.iter().map(|f| f.path.clone()).collect();
    let mut sorted = listed.clone();
    sorted.sort();
    assert_eq!(listed, sorted);
    assert_eq!(first.total_matches, 122);
    assert_eq!(first.files_searched, 122);
}

#[test]
fn files_only_lists_paths_with_counts() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    write(root, "a.txt", "hit\nhit\n");
    write(root, "b.txt", "miss\n");
    write(root, "c.txt", "hit\n");

    let mut opts = GrepOptions::new("hit");
    opts.files_only = true;
    let report = search(root, &opts);
    assert_eq!(paths(&report), ["a.txt", "c.txt"]);
    assert!(report.files.iter().all(|f| f.matches.is_empty()));
    assert_eq!((report.total_matches, report.files_with_matches), (3, 2));
    let text = render(&report, 1500);
    assert!(text.starts_with("a.txt (2)\nc.txt (1)\n# grep: 3 match(es) in 2 file(s)"), "{text}");
}

#[test]
fn render_groups_by_file_and_reports_totals() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    write(root, "a.txt", "one needle\n");
    write(root, "b.txt", "x\ntwo needle\n");

    let text = render(&search(root, &GrepOptions::new("needle")), 1500);
    assert_eq!(
        text,
        "a.txt\n  1: one needle\nb.txt\n  2: two needle\n# grep: 2 of 2 match(es) in 2 file(s), searched 2 file(s), backend=builtin"
    );
    let none = render(&search(root, &GrepOptions::new("absent")), 1500);
    assert_eq!(none, "# grep: no matches, searched 2 file(s), backend=builtin");
}
