//! Shell commands: `rg`, `grep`, `git grep`, `anvil grep`, ... searching the repository, and the
//! `anvil` and file-reading commands that consult the graph or docs.

use std::path::Path;

use super::super::super::roles_shell::{AnvilArgs, anvil_subcommand};
use super::super::super::shell::destructive::split_git_subcommand;
use super::super::super::shell::expand::{Invocation, expand};
use super::{Call, is_docs_path, query_of_pattern, search_call};

/// Programs that print a file's content; reading a concept this way consults the docs.
const READERS: &[&str] =
    &["cat", "head", "tail", "less", "more", "bat", "type", "get-content", "gc"];
/// Programs searching text with a `pattern [path...]` command line.
const PATTERN_SEARCHERS: &[&str] = &["rg", "grep", "egrep", "fgrep", "ag", "ack", "tgrep"];
/// Options of the searchers above that take their value as the next word.
const VALUE_FLAGS: &[&str] = &[
    "-A",
    "-B",
    "-C",
    "-m",
    "-g",
    "-t",
    "-T",
    "-j",
    "-d",
    "-D",
    "-f",
    "-M",
    "--glob",
    "--iglob",
    "--type",
    "--type-not",
    "--threads",
    "--max-count",
    "--max-depth",
    "--maxdepth",
    "--file",
    "--include",
    "--exclude",
    "--exclude-dir",
    "--include-dir",
    "--context",
    "--after-context",
    "--before-context",
    "--color",
    "--colour",
    "--sort",
    "--sortr",
    "--max-columns",
    "--pre",
    "--ignore-file",
    "--max-matches",
    "--max-per-file",
    "--max-tokens",
    "--backend",
];
/// `Select-String` switches; every other `-Parameter` takes a value.
const PS_SWITCHES: &[&str] =
    &["-simplematch", "-casesensitive", "-list", "-quiet", "-allmatches", "-notmatch", "-raw"];

/// Classifies a command line: a consultation anywhere wins, else the first repository search.
pub fn classify(root: &Path, cmd: &str) -> Call {
    let mut search = Call::Other;
    for inv in &expand(cmd).invocations {
        if consults(inv) {
            return Call::Consult;
        }
        if matches!(search, Call::Other) {
            search = search_of(root, inv);
        }
    }
    search
}

/// `anvil search|graph|context|docs search`, or reading a file under `.harness/docs/`.
fn consults(inv: &Invocation) -> bool {
    if inv.name == "anvil" {
        return match anvil_subcommand(&inv.args) {
            AnvilArgs::Sub(sub, rest) => {
                matches!(sub, "search" | "graph" | "context")
                    || (sub == "docs" && rest.first().is_some_and(|a| a == "search"))
            }
            _ => false,
        };
    }
    READERS.contains(&inv.name.as_str()) && inv.args.iter().any(|a| is_docs_path(a))
}

fn search_of(root: &Path, inv: &Invocation) -> Call {
    if inv.fed_by_pipe || !inv.inputs.is_empty() {
        return Call::Other;
    }
    let name = inv.name.as_str();
    let operands = match name {
        "git" => split_git_subcommand(&inv.args)
            .filter(|(sub, _)| *sub == "grep")
            .map(|(_, r)| posix(name, r)),
        "anvil" => match anvil_subcommand(&inv.args) {
            AnvilArgs::Sub("grep", rest) => Some(posix(name, rest)),
            _ => None,
        },
        "findstr" => Some(findstr(&inv.args)),
        "select-string" | "sls" => Some(select_string(&inv.args)),
        _ if PATTERN_SEARCHERS.contains(&name) => Some(posix(name, &inv.args)),
        _ => None,
    };
    match operands {
        Some((Some(pattern), paths)) => search_call(root, &query_of_pattern(&pattern), &paths),
        _ => Call::Other,
    }
}

/// `(pattern, paths)`; the pattern is `None` when the command lists files instead of searching.
type Operands = (Option<String>, Vec<String>);

/// Splits a `grep`/`rg`-style command line into its pattern and path operands.
fn posix(name: &str, args: &[String]) -> Operands {
    let mut explicit: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut lists_files = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            positional.extend(iter.by_ref().cloned());
        } else if arg == "--files" || arg == "--type-list" {
            lists_files = true;
        } else if let Some(value) = arg.strip_prefix("--regexp=") {
            explicit = Some(value.to_owned());
        } else if arg == "--regexp" || arg == "-e" || ends_cluster_with(arg, 'e') {
            explicit = iter.next().cloned().or(explicit);
        } else if let Some(value) =
            arg.strip_prefix("-e").filter(|v| !v.is_empty() && !arg.starts_with("--"))
        {
            explicit = Some(value.to_owned());
        } else if takes_value(name, arg) {
            iter.next();
        } else if !arg.starts_with('-') || arg == "-" {
            positional.push(arg.clone());
        }
    }
    if lists_files {
        return (None, positional);
    }
    match explicit {
        Some(pattern) => (Some(pattern), positional),
        None if positional.is_empty() => (None, positional),
        None => (Some(positional.remove(0)), positional),
    }
}

/// A short cluster such as `-rne` whose last letter is the `-e` option.
fn ends_cluster_with(arg: &str, letter: char) -> bool {
    arg.len() > 2
        && arg.starts_with('-')
        && !arg.starts_with("--")
        && arg.ends_with(letter)
        && arg[1..].chars().all(|c| c.is_ascii_alphabetic())
}

fn takes_value(name: &str, arg: &str) -> bool {
    if arg.contains('=') {
        return false;
    }
    if name == "rg" && matches!(arg, "-r" | "--replace" | "-E" | "--encoding") {
        return true;
    }
    if VALUE_FLAGS.contains(&arg) {
        return true;
    }
    // A short cluster such as `-nA` whose last letter takes the next word as its value.
    arg.len() > 2
        && arg.starts_with('-')
        && !arg.starts_with("--")
        && arg[1..].chars().all(|c| c.is_ascii_alphabetic())
        && arg.chars().last().is_some_and(|c| {
            matches!(c, 'A' | 'B' | 'C' | 'm' | 'g' | 't' | 'T' | 'j' | 'd' | 'f' | 'M')
        })
}

/// `findstr [/flags] pattern [files]`; `/C:text` carries the pattern inside the flag.
fn findstr(args: &[String]) -> Operands {
    let mut explicit: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    for arg in args {
        let lower = arg.to_ascii_lowercase();
        if let Some(text) = lower.strip_prefix("/c:").map(|_| arg[3..].to_owned()) {
            explicit = Some(text);
        } else if is_findstr_flag(arg) {
            continue;
        } else {
            positional.push(arg.clone());
        }
    }
    match explicit {
        Some(pattern) => (Some(pattern), positional),
        None if positional.is_empty() => (None, positional),
        None => (Some(positional.remove(0)), positional),
    }
}

/// `/S`, `/I`, `/N`, `/D:dirs`; a longer slash word is a path such as `/tmp/x`.
fn is_findstr_flag(arg: &str) -> bool {
    arg.starts_with('/') && (arg.len() == 2 || arg.get(2..3) == Some(":"))
}

/// `Select-String [-Pattern] pattern [[-Path] path] [-Switch] [-Parameter value]`.
fn select_string(args: &[String]) -> Operands {
    let mut pattern: Option<String> = None;
    let mut paths: Vec<String> = Vec::new();
    let mut positional: Vec<String> = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let lower = arg.to_ascii_lowercase();
        if !lower.starts_with('-') || arg == "-" {
            positional.push(arg.clone());
        } else if lower == "-pattern" {
            pattern = iter.next().cloned();
        } else if lower == "-path" || lower == "-literalpath" {
            paths.extend(iter.next().iter().flat_map(|v| v.split(',')).map(str::to_owned));
        } else if !PS_SWITCHES.contains(&lower.as_str()) {
            iter.next();
        }
    }
    let mut positional = positional.into_iter();
    let pattern = pattern.or_else(|| positional.next());
    paths.extend(positional.flat_map(|v| v.split(',').map(str::to_owned).collect::<Vec<_>>()));
    (pattern, paths)
}
