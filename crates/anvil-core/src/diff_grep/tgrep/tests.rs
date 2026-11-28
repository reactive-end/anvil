//! tgrep backend tests: JSON stream parsing, report building, status detection, backend selection.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;
use crate::config::search::SearchBackend;
use crate::diff_grep::grep_with_binary;
use crate::diff_grep::tgrep_parse::parse_json_stream;

/// Captured from `tgrep --json -C1 "fn "` (1.0.5), with an extra non-UTF-8 record and a noise file.
const STREAM: &str = r#"{"data":{"path":{"text":".\\src\\a.rs"}},"type":"begin"}
{"data":{"absolute_offset":0,"line_number":1,"lines":{"text":"fn main() {\n"},"path":{"text":".\\src\\a.rs"},"submatches":[{"end":3,"match":{"text":"fn "},"start":0}]},"type":"match"}
{"data":{"absolute_offset":12,"line_number":2,"lines":{"text":"  let x = 1;\n"},"path":{"text":".\\src\\a.rs"},"submatches":[]},"type":"context"}
{"data":{"absolute_offset":25,"line_number":3,"lines":{"text":"}\n"},"path":{"text":".\\src\\a.rs"},"submatches":[]},"type":"context"}
{"data":{"absolute_offset":27,"line_number":4,"lines":{"text":"fn other() {}\n"},"path":{"text":".\\src\\a.rs"},"submatches":[{"end":3,"match":{"text":"fn "},"start":0}]},"type":"match"}
{"data":{"binary_offset":null,"path":{"text":".\\src\\a.rs"},"stats":{"matches":2}},"type":"end"}
{"data":{"path":{"text":"b.txt"}},"type":"begin"}
{"data":{"absolute_offset":0,"line_number":2,"lines":{"text":"fn z()\n"},"path":{"text":"b.txt"},"submatches":[]},"type":"match"}
{"data":{"path":{"text":"b.txt"},"stats":{}},"type":"end"}
{"data":{"path":{"text":"node_modules/dep/index.js"}},"type":"begin"}
{"data":{"line_number":1,"lines":{"text":"fn noisy()\n"},"path":{"text":"node_modules/dep/index.js"},"submatches":[]},"type":"match"}
{"data":{"path":{"text":"node_modules/dep/index.js"},"stats":{}},"type":"end"}
{"data":{"path":{"bytes":"//79"}},"type":"begin"}
{"data":{"line_number":1,"lines":{"bytes":"//79"},"path":{"bytes":"//79"},"submatches":[]},"type":"match"}
{"data":{"path":{"bytes":"//79"},"stats":{}},"type":"end"}
{"data":{"elapsed_total":{"human":"0.003s","nanos":3,"secs":0},"stats":{"searches":4,"matches":4}},"type":"summary"}
"#;

fn opts(pattern: &str) -> GrepOptions {
    GrepOptions::new(pattern)
}

#[test]
fn parser_extracts_matches_context_and_summary() {
    let stream = parse_json_stream(STREAM).expect("parse");
    assert_eq!(stream.searches, Some(4));
    assert_eq!(stream.files.len(), 3, "non-UTF-8 path records are dropped");
    let first = &stream.files[0];
    assert_eq!(first.path, r".\src\a.rs");
    assert_eq!(first.matches, [(1, "fn main() {\n".to_owned()), (4, "fn other() {}\n".to_owned())]);
    assert_eq!(first.context.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [2, 3]);
    assert!(parse_json_stream("not json").is_err());
    assert_eq!(parse_json_stream("\n\n").expect("blank"), Default::default());
}

#[test]
fn report_applies_noise_filter_normalizes_paths_and_attaches_context() {
    let stream = parse_json_stream(STREAM).expect("parse");
    let mut options = opts("fn ");
    options.context = 1;
    let report = build_report(Path::new("/repo"), &options, stream);

    assert_eq!(report.backend, "tgrep");
    assert_eq!(
        report.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        ["b.txt", "src/a.rs"]
    );
    assert_eq!(report.total_matches, 3, "node_modules hit is filtered out");
    assert_eq!(report.files_searched, 4);
    let a = &report.files[1];
    assert_eq!(a.matches[0].text, "fn main() {");
    assert_eq!(a.matches[0].after, [(2, "let x = 1;".to_owned())]);
    assert_eq!(a.matches[1].before, [(3, "}".to_owned())]);
    assert!(a.matches[1].after.is_empty() && a.matches[0].before.is_empty());
}

#[test]
fn report_drops_paths_the_builtin_engine_never_searches() {
    let stream = || ParsedStream {
        files: [
            ".github/workflows/ci.yml",
            ".harness/rules/r.md",
            ".harness/tasks/T-1/progress.md",
            ".harness/memory/chunks/c.json",
            ".secret/x.txt",
            ".git/config",
            "src/a.rs",
        ]
        .map(|path| ParsedFile {
            path: path.to_owned(),
            matches: vec![(1, "needle".to_owned())],
            context: Vec::new(),
        })
        .into(),
        searches: None,
    };
    let paths = |hidden: bool| {
        let mut options = opts("needle");
        options.hidden = hidden;
        let report = build_report(Path::new("/repo"), &options, stream());
        report.files.into_iter().map(|f| f.path).collect::<Vec<_>>()
    };
    assert_eq!(paths(false), [".github/workflows/ci.yml", ".harness/rules/r.md", "src/a.rs"]);
    assert_eq!(
        paths(true),
        [".github/workflows/ci.yml", ".harness/rules/r.md", ".secret/x.txt", "src/a.rs"]
    );
}

#[test]
fn report_applies_the_same_caps_as_the_builtin_engine() {
    let stream = parse_json_stream(STREAM).expect("parse");
    let mut options = opts("fn ");
    options.max_matches = 2;
    options.max_per_file = 1;
    let report = build_report(Path::new("/repo"), &options, stream);
    assert_eq!(report.total_matches, 3);
    assert_eq!(report.files.iter().map(|f| f.matches.len()).sum::<usize>(), 2);
    assert_eq!(report.files[1].total, 2);
    assert!(report.truncated);
}

#[test]
fn arguments_mirror_the_options() {
    let tmp = TempDir::new().expect("tempdir");
    let mut options = opts("-x");
    options.ignore_case = true;
    options.fixed_strings = true;
    options.word = true;
    options.hidden = true;
    options.context = 3;
    options.globs = vec!["*.rs".into()];
    options.paths = vec!["src".into()];
    let args = build_args(tmp.path(), &options);
    assert_eq!(&args[..2], ["--json", "--no-config"]);
    for flag in ["-i", "-F", "-w", "--hidden", "-g", "*.rs", "-C", "3"] {
        assert!(args.iter().any(|a| a == flag), "missing {flag}: {args:?}");
    }
    assert!(!args.iter().any(|a| a == "-S" || a == "--index-path"), "{args:?}");
    assert_eq!(args[args.len() - 3..], ["--", "-x", "src"]);

    fs::create_dir(tmp.path().join(".tgrep")).expect("index dir");
    let smart = build_args(tmp.path(), &opts("foo"));
    for flag in [
        "--hidden",
        "!.git/",
        "!.tgrep/",
        "!/.harness/tasks/",
        "!/.harness/memory/",
        "!/.harness/candidates/",
    ] {
        assert!(smart.iter().any(|a| a == flag), "missing {flag}: {smart:?}");
    }
    assert!(smart.iter().any(|a| a == "-S") && smart.iter().any(|a| a == "--index-path"));
    let mut sensitive = opts("foo");
    sensitive.smart_case = false;
    assert!(build_args(tmp.path(), &sensitive).iter().any(|a| a == "-s"));
}

#[test]
fn status_detection_requires_a_live_watched_server() {
    let running = "Server status for C:\\repo\n  PID:        1\n  Watcher:    active\n  Indexing:   complete\n  Reconcile overdue: no\n";
    assert!(status_shows_fresh_server(running));
    let stopped = "Index status for C:\\repo\n  Files:      2\n  Server:     not running\n";
    assert!(!status_shows_fresh_server(stopped));
    assert!(!status_shows_fresh_server(&running.replace("active", "disabled")));
    assert!(!status_shows_fresh_server(&running.replace("complete", "in progress")));
    assert!(!status_shows_fresh_server(&running.replace("overdue: no", "overdue: yes")));
    assert!(!status_shows_fresh_server("No index found at C:\\repo\\.tgrep\n"));
}

#[test]
fn auto_uses_builtin_without_an_index_and_tgrep_falls_back_with_a_note() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    fs::write(root.join("a.txt"), "needle\n").expect("write");
    let fake_binary = root.join("definitely-missing-tgrep");

    let auto = grep_with_binary(root, &opts("needle"), SearchBackend::Auto, Some(&fake_binary))
        .expect("auto");
    assert_eq!((auto.backend.as_str(), auto.note.as_deref()), ("builtin", None));
    assert!(!auto_eligible(root, Some(&fake_binary)));

    fs::create_dir(root.join(".tgrep")).expect("index dir");
    assert!(!auto_eligible(root, None), "no binary on PATH");
    assert!(!auto_eligible(root, Some(&fake_binary)), "binary cannot run");
    let still_builtin =
        grep_with_binary(root, &opts("needle"), SearchBackend::Auto, None).expect("auto 2");
    assert_eq!(still_builtin.backend, "builtin");

    for binary in [None, Some(fake_binary.as_path())] {
        let forced = grep_with_binary(root, &opts("needle"), SearchBackend::Tgrep, binary)
            .expect("forced tgrep falls back");
        assert_eq!(forced.backend, "builtin");
        assert_eq!(forced.total_matches, 1);
        let note = forced.note.expect("fallback note");
        assert!(note.contains("tgrep unavailable") && note.contains("builtin"), "{note}");
    }
}

#[test]
fn builtin_never_creates_an_index() {
    let tmp = TempDir::new().expect("tempdir");
    fs::write(tmp.path().join("a.txt"), "needle\n").expect("write");
    for backend in [SearchBackend::Auto, SearchBackend::Builtin, SearchBackend::Tgrep] {
        grep_with_binary(tmp.path(), &opts("needle"), backend, None).expect("grep");
    }
    assert!(!tmp.path().join(".tgrep").exists());
}
