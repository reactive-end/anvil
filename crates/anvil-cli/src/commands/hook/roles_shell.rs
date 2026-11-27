//! Shell policies for orchestrator mode: an allowlist for judging, a denylist for executing.

use super::payload::should_wrap_with_anvil_run;
use super::roles::orchestrator_cmds::{is_orchestrator_gh, is_orchestrator_git};
use super::roles::writes::is_discard;
use super::shell::command_name;
use super::shell::destructive::split_git_subcommand;
use super::shell::expand::expand;
use super::shell::lex::{Segment, parse};

const INSPECTORS: &[&str] = &[
    "ls",
    "dir",
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "nl",
    "wc",
    "sort",
    "uniq",
    "cut",
    "tr",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "fd",
    "tree",
    "stat",
    "file",
    "du",
    "pwd",
    "echo",
    "printf",
    "test",
    "[",
    "true",
    "false",
    "which",
    "where",
    "whereis",
    "date",
    "diff",
    "cmp",
    "jq",
    "cd",
    "pushd",
    "popd",
    "sleep",
    "basename",
    "dirname",
    "realpath",
    "for",
    "in",
    "done",
    "fi",
    "read",
    "get-content",
    "gc",
    "get-childitem",
    "gci",
    "get-item",
    "get-location",
    "set-location",
    "select-string",
    "sls",
    "select-object",
    "measure-object",
    "sort-object",
    "where-object",
    "format-table",
    "format-list",
    "test-path",
    "resolve-path",
    "write-output",
    "write-host",
    "findstr",
    "type",
];
const GIT_READERS: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "blame",
    "ls-files",
    "ls-tree",
    "rev-parse",
    "describe",
    "shortlog",
    "grep",
    "cat-file",
    "merge-base",
    "rev-list",
    "diff-tree",
    "name-rev",
];
const ANVIL_READERS: &[&str] = &[
    "check", "diff", "grep", "impact", "graph", "mem", "tasks", "stats", "doctor", "deps",
    "context", "code", "run", "sonar", "qa",
];
/// `anvil guard` subcommands that change the security guard; `list` and `test` only read it.
const GUARD_WRITERS: &[&str] = &["set", "add", "remove", "reset"];
/// `anvil docs` subcommands that only read; `verify` records a human review and is the user's alone.
const DOCS_READERS: &[&str] = &["list", "check", "search"];
const KEYWORDS: &[&str] = &["if", "then", "else", "elif", "do", "while", "until", "!"];
const SELF_FIXING_FLAGS: &[&str] =
    &["--fix", "--write", "--unsafe", "--apply", "--update", "--updatesnapshot", "-u", "--format"];

/// Explains why `cmd` is not a pure inspection or verification command.
pub fn why_not_read_only(cmd: &str) -> Option<String> {
    let script = parse(cmd);
    if let Some(reason) = script.nested.iter().find_map(|nested| why_not_read_only(nested)) {
        return Some(reason);
    }
    script.segments.iter().find_map(|seg| {
        if let Some(out) = seg.outputs.iter().find(|out| !is_discard(out)) {
            return Some(format!("writing to `{out}` from the shell is not allowed"));
        }
        (!is_read_only_segment(seg)).then(|| {
            format!("`{}` is not an inspection or verification command", seg.args.join(" "))
        })
    })
}

/// Explains why an executor may not run `cmd`: it would take over the orchestrator's decisions.
pub fn why_not_executor(cmd: &str) -> Option<String> {
    expand(cmd).invocations.iter().find_map(|inv| {
        let (name, sub) = match inv.name.as_str() {
            "git" => (
                "git",
                split_git_subcommand(&inv.args)
                    .map(|(sub, _)| sub)
                    .filter(|s| is_reserved_git(s))?,
            ),
            "anvil" => ("anvil", reserved_anvil(&inv.args)?),
            _ => return None,
        };
        Some(format!("`{name} {sub}` belongs to the orchestrator; report back instead"))
    })
}

fn is_reserved_git(sub: &str) -> bool {
    matches!(
        sub,
        "commit"
            | "push"
            | "merge"
            | "rebase"
            | "reset"
            | "checkout"
            | "switch"
            | "restore"
            | "stash"
            | "branch"
            | "tag"
            | "cherry-pick"
            | "revert"
            | "am"
            | "apply"
            | "clean"
    )
}

fn is_reserved_anvil(sub: &str, rest: &[String]) -> bool {
    if asks_for_help(sub, rest) {
        return false;
    }
    let mentions = |words: &[&str]| rest.iter().any(|a| words.contains(&a.as_str()));
    matches!(sub, "orchestrator" | "init" | "sync" | "hook")
        || (sub == "plugin" && rest.first().is_none_or(|a| a != "status"))
        || (sub == "guard" && rest.first().is_some_and(|a| GUARD_WRITERS.contains(&a.as_str())))
        || (sub == "docs" && is_docs_verify(rest))
        || (sub == "circuit" && mentions(&["reset"]))
        || (sub == "ratchet" && mentions(&["promote"]))
        || (sub == "tasks" && mentions(&["start", "done", "skip", "record"]))
        || (sub == "review" && mentions(&["approve"]))
}

/// `anvil docs verify`: the first word after `docs`, or any word when a global option comes first.
fn is_docs_verify(rest: &[String]) -> bool {
    match rest.first() {
        Some(first) if first.starts_with('-') => rest.iter().any(|a| a == "verify"),
        Some(first) => first == "verify",
        None => false,
    }
}

/// `anvil help ...` and `anvil <subcommand ...> --help|-h` print usage and never write. Words after
/// `--` belong to a wrapped command, and `anvil run` hands every word after its first to that command.
fn asks_for_help(sub: &str, rest: &[String]) -> bool {
    let is_flag = |arg: &String| arg == "--help" || arg == "-h";
    if sub == "help" {
        return true;
    }
    if sub == "run" {
        return rest.first().is_some_and(is_flag);
    }
    rest.iter().take_while(|arg| *arg != "--").any(is_flag)
}

/// The reserved `anvil` subcommand in `args`, if any. When the words before the subcommand cannot be
/// parsed, every later word is tried as the subcommand so an unknown flag cannot hide one.
fn reserved_anvil(args: &[String]) -> Option<&str> {
    match anvil_subcommand(args) {
        AnvilArgs::Sub(sub, rest) => is_reserved_anvil(sub, rest).then_some(sub),
        AnvilArgs::NoSub => None,
        AnvilArgs::Unparsed => (0..args.len())
            .find(|&i| is_reserved_anvil(&args[i], &args[i + 1..]))
            .map(|i| args[i].as_str()),
    }
}

/// Outcome of skipping the global options in front of an `anvil` subcommand.
enum AnvilArgs<'a> {
    Sub(&'a str, &'a [String]),
    NoSub,
    /// An option that is not a known global one came first.
    Unparsed,
}

/// Skips the global options declared in `cli.rs` (`--root <PATH>`, `--root=<PATH>`, `--no-color`) and
/// clap's own `--help`/`--version`; none of them has a short form besides `-h`/`-V`.
fn anvil_subcommand(args: &[String]) -> AnvilArgs<'_> {
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        match arg.as_str() {
            "--root" => i += 2,
            "--no-color" | "--help" | "--version" | "-h" | "-V" => i += 1,
            flag if flag.starts_with("--root=") => i += 1,
            flag if flag.starts_with('-') => return AnvilArgs::Unparsed,
            sub => return AnvilArgs::Sub(sub, &args[i + 1..]),
        }
    }
    AnvilArgs::NoSub
}

fn is_read_only_segment(seg: &Segment) -> bool {
    let words: Vec<&String> =
        seg.args.iter().skip_while(|w| KEYWORDS.contains(&w.as_str())).collect();
    let Some((head, rest)) = words.split_first() else {
        return true;
    };
    let args: Vec<String> = rest.iter().map(|w| (*w).clone()).collect();
    match command_name(head).as_str() {
        "git" => is_orchestrator_git(&args, GIT_READERS),
        "gh" => is_orchestrator_gh(&args),
        "anvil" => is_read_only_anvil(&args),
        "find" => !args.iter().any(|a| {
            a.starts_with("-exec")
                || a.starts_with("-ok")
                || a.starts_with("-fprint")
                || a == "-delete"
        }),
        "sed" => {
            args.first().is_some_and(|a| a == "-n")
                && args.get(1).is_some_and(|s| is_print_range(s))
        }
        name if INSPECTORS.contains(&name) => !writes_with_flag(name, &args),
        _ => is_verification(&words),
    }
}

/// `sort -o`, `tree -o`, and `fd -x` turn inspectors into writers or launchers.
fn writes_with_flag(name: &str, args: &[String]) -> bool {
    let flags: &[&str] = match name {
        "sort" | "tree" => &["-o", "--output"],
        "fd" => &["-x", "-X", "--exec", "--exec-batch"],
        "rg" => &["--pre"],
        "less" => &["-o", "-O"],
        _ => return false,
    };
    args.iter().any(|a| flags.iter().any(|flag| a.starts_with(flag)))
}

fn is_print_range(script: &str) -> bool {
    script.ends_with('p')
        && script.chars().all(|c| c.is_ascii_digit() || matches!(c, ',' | '$' | 'p' | ';'))
}

fn is_read_only_anvil(args: &[String]) -> bool {
    let (sub, rest) = match anvil_subcommand(args) {
        AnvilArgs::Unparsed => return false,
        AnvilArgs::NoSub => return args.iter().any(|a| a == "--version" || a == "--help"),
        AnvilArgs::Sub(sub, rest) => (sub, rest),
    };
    if asks_for_help(sub, rest) {
        return true;
    }
    match sub {
        "run" => {
            is_verification(&rest.iter().skip_while(|a| a.as_str() == "--").collect::<Vec<_>>())
        }
        "orchestrator" | "circuit" | "plugin" => rest.first().is_some_and(|a| a == "status"),
        "ratchet" => rest.first().is_some_and(|a| a != "promote"),
        "checkpoint" | "worktree" => rest.first().is_some_and(|a| a == "list"),
        "guard" => rest.first().is_some_and(|a| a == "list" || a == "test"),
        "docs" => rest.first().is_some_and(|a| DOCS_READERS.contains(&a.as_str())),
        sub => ANVIL_READERS.contains(&sub),
    }
}

/// Test, lint, typecheck, and build commands that do not rewrite sources.
fn is_verification(words: &[&String]) -> bool {
    let joined = words.iter().map(|w| w.as_str()).collect::<Vec<_>>().join(" ");
    let rewrites =
        words.iter().any(|w| SELF_FIXING_FLAGS.contains(&w.to_ascii_lowercase().as_str()));
    let formats =
        words.iter().any(|w| matches!(command_name(w).as_str(), "pint" | "godot" | "godot4"));
    !rewrites && !formats && should_wrap_with_anvil_run(&joined)
}

#[cfg(test)]
mod tests;
