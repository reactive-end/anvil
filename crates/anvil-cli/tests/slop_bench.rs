//! Slop benchmark: runs `anvil check` on a corpus of small changes and compares what it reports
//! with what each case expects.
//!
//! Each `crates/anvil-cli/slop_bench/*.case` file holds one change (kept outside `tests/`, since
//! the cases are data and test-tampering checks treat everything under `tests/` as a test):
//!
//! ```text
//! # what the case is about
//! @@ before src/a.ts      file committed before the change
//! @@ after src/a.ts       file content after the change (uncommitted)
//! @@ expect               one `<error|warn> <rule> <path>` line per finding; empty means clean
//! @@ known-gap            findings Anvil should report but does not yet, same line format
//! ```
//!
//! The test fails on any missed or unexpected finding, and when a known gap starts being
//! reported, so the case is moved to `expect`. `--nocapture` prints the per-rule table.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

type Finding = (String, String, String);

#[derive(Default)]
struct Case {
    name: String,
    before: Vec<(String, String)>,
    after: Vec<(String, String)>,
    expect: Vec<Finding>,
    known_gaps: Vec<Finding>,
}

#[derive(Default)]
struct Tally {
    caught: usize,
    missed: usize,
    false_positives: usize,
    known_gaps: usize,
}

fn parse_case(name: &str, text: &str) -> Case {
    let mut case = Case { name: name.to_owned(), ..Case::default() };
    let mut section: Option<(String, Option<String>)> = None;
    let mut body = String::new();
    let flush = |section: &Option<(String, Option<String>)>, body: &str, case: &mut Case| {
        let Some((kind, path)) = section else { return };
        match (kind.as_str(), path) {
            ("before", Some(path)) => case.before.push((path.clone(), body.to_owned())),
            ("after", Some(path)) => case.after.push((path.clone(), body.to_owned())),
            ("expect", None) => case.expect.extend(parse_findings(name, body)),
            ("known-gap", None) => case.known_gaps.extend(parse_findings(name, body)),
            _ => panic!("{name}: bad section header `@@ {kind}`"),
        }
    };
    for line in text.replace("\r\n", "\n").split_inclusive('\n') {
        if let Some(header) = line.strip_prefix("@@ ") {
            flush(&section, &body, &mut case);
            body.clear();
            let mut parts = header.split_whitespace();
            let kind = parts.next().expect("section kind").to_owned();
            section = Some((kind, parts.next().map(str::to_owned)));
        } else if section.is_some() {
            body.push_str(line);
        }
    }
    flush(&section, &body, &mut case);
    case
}

fn parse_findings(name: &str, body: &str) -> Vec<Finding> {
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            assert!(
                parts.len() == 3 && ["error", "warn"].contains(&parts[0]),
                "{name}: expected `<error|warn> <rule> <path>`, got `{line}`"
            );
            (parts[0].to_owned(), parts[1].to_owned(), parts[2].to_owned())
        })
        .collect()
}

fn write_files(root: &Path, files: &[(String, String)]) {
    for (rel, content) in files {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, content).expect("write case file");
    }
}

fn run(root: &Path, program: &str, args: &[&str]) -> Output {
    let out = Command::new(program).args(args).current_dir(root).output().expect("spawn");
    assert!(
        out.status.success() || program != "git",
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Builds the case's repository and returns the findings `anvil check` printed.
fn check_case(case: &Case) -> Vec<Finding> {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let anvil = env!("CARGO_BIN_EXE_anvil");
    run(root, "git", &["init", "-q"]);
    run(root, "git", &["config", "user.email", "bench@example.com"]);
    run(root, "git", &["config", "user.name", "bench"]);
    write_files(root, &case.before);
    let init = run(root, anvil, &["init"]);
    assert!(init.status.success(), "{}: anvil init failed", case.name);
    run(root, "git", &["add", "-A"]);
    run(root, "git", &["commit", "-q", "-m", "before"]);
    write_files(root, &case.after);
    let out = run(root, anvil, &["check", "--no-tests", "--no-color"]);
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    text.lines().filter_map(parse_reported).collect()
}

/// Reads `[ERROR] path:l:c [rule] …` and `[ERROR] [rule] path:l — …` lines.
fn parse_reported(line: &str) -> Option<Finding> {
    let line = line.trim_start();
    let (level, rest) = if let Some(rest) = line.strip_prefix("[ERROR] ") {
        ("error", rest)
    } else {
        ("warn", line.strip_prefix("[WARN] ")?)
    };
    let (rule, path) = if let Some(tagged) = rest.strip_prefix('[') {
        let (rule, tail) = tagged.split_once("] ")?;
        (rule, tail.split_whitespace().next()?)
    } else {
        let (location, tail) = rest.split_once(" [")?;
        (tail.split_once(']')?.0, location)
    };
    let path = path.split(':').next()?.replace('\\', "/");
    Some((level.to_owned(), rule.to_owned(), path))
}

/// Checks every case, a few at a time, keeping the cases' order.
fn check_all(cases: &[Case]) -> Vec<Vec<Finding>> {
    let width = std::thread::available_parallelism().map_or(4, usize::from);
    cases
        .chunks(width)
        .flat_map(|chunk| {
            std::thread::scope(|scope| {
                let running: Vec<_> =
                    chunk.iter().map(|case| scope.spawn(|| check_case(case))).collect();
                running
                    .into_iter()
                    .map(|handle| handle.join().expect("case thread"))
                    .collect::<Vec<_>>()
            })
        })
        .collect()
}

fn load_cases() -> Vec<Case> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("slop_bench");
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("slop_bench dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "case"))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|path| {
            let name = path.file_stem().expect("stem").to_string_lossy().into_owned();
            parse_case(&name, &fs::read_to_string(path).expect("read case"))
        })
        .collect()
}

fn print_table(tallies: &BTreeMap<String, Tally>, cases: usize, clean: usize) {
    println!("\nslop benchmark: {cases} case(s), {clean} clean");
    println!("{:<34} {:>6} {:>6} {:>6} {:>5}", "rule", "caught", "missed", "false+", "gap");
    for (rule, t) in tallies {
        println!(
            "{rule:<34} {:>6} {:>6} {:>6} {:>5}",
            t.caught, t.missed, t.false_positives, t.known_gaps
        );
    }
}

#[test]
fn slop_benchmark_matches_every_case() {
    let cases = load_cases();
    assert!(!cases.is_empty(), "no slop_bench cases found");
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    let mut failures = Vec::new();
    for (case, mut reported) in cases.iter().zip(check_all(&cases)) {
        for expected in &case.expect {
            let tally = tallies.entry(expected.1.clone()).or_default();
            if let Some(at) = reported.iter().position(|r| r == expected) {
                reported.remove(at);
                tally.caught += 1;
            } else {
                tally.missed += 1;
                failures.push(format!("{}: missed {expected:?}", case.name));
            }
        }
        for gap in &case.known_gaps {
            tallies.entry(gap.1.clone()).or_default().known_gaps += 1;
            if let Some(at) = reported.iter().position(|r| r == gap) {
                reported.remove(at);
                failures.push(format!("{}: known gap {gap:?} is now reported", case.name));
            }
        }
        for extra in reported {
            tallies.entry(extra.1.clone()).or_default().false_positives += 1;
            failures.push(format!("{}: unexpected {extra:?}", case.name));
        }
    }
    let clean = cases.iter().filter(|c| c.expect.is_empty() && c.known_gaps.is_empty()).count();
    print_table(&tallies, cases.len(), clean);
    assert!(failures.is_empty(), "slop benchmark mismatches:\n{}", failures.join("\n"));
}

#[test]
fn reported_lines_parse_in_both_formats() {
    let tier0 = "  [ERROR] src/math.ts:12:4 [no-todo-comments] Disallowed `TODO` marker";
    let tier1 = "  [ERROR] [overengineering:unused-new-code] src/math.ts:9 — exported `x` is new";
    let warn = "  [WARN] src\\a.rs:1:1 [no-narrative-comments] Long narrative comment";
    let finding = |l: &str, r: &str, p: &str| Some((l.to_owned(), r.to_owned(), p.to_owned()));
    assert_eq!(parse_reported(tier0), finding("error", "no-todo-comments", "src/math.ts"));
    assert_eq!(
        parse_reported(tier1),
        finding("error", "overengineering:unused-new-code", "src/math.ts")
    );
    assert_eq!(parse_reported(warn), finding("warn", "no-narrative-comments", "src/a.rs"));
    assert_eq!(parse_reported("  [Tier 1] blast_radius: 1 impacted"), None);
}
