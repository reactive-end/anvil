//! Shell policies for orchestrator mode: an allowlist for judging, a denylist for executing.

use super::payload::should_wrap_with_anvil_run;
use super::shell::command_name;
use super::shell::destructive::split_git_subcommand;
use super::shell::expand::{Invocation, expand};
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
    let mentions = |words: &[&str]| rest.iter().any(|a| words.contains(&a.as_str()));
    matches!(sub, "orchestrator" | "init" | "sync" | "hook")
        || (sub == "plugin" && rest.first().is_none_or(|a| a != "status"))
        || (sub == "circuit" && mentions(&["reset"]))
        || (sub == "ratchet" && mentions(&["promote"]))
        || (sub == "tasks" && mentions(&["start", "done", "skip", "record"]))
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

/// Lists the paths `cmd` visibly writes: redirections and operands of file-writing commands.
pub fn written_paths(cmd: &str) -> Vec<String> {
    expand(cmd).invocations.iter().flat_map(invocation_writes).collect()
}

fn invocation_writes(inv: &Invocation) -> Vec<String> {
    let mut paths: Vec<String> =
        inv.outputs.iter().filter(|out| !is_discard(out)).cloned().collect();
    let operands = || inv.args.iter().filter(|a| !a.starts_with('-')).cloned();
    match inv.name.as_str() {
        "rm" | "touch" | "tee" | "truncate" | "mkdir" | "rmdir" | "unlink" | "del" | "erase"
        | "ri" | "remove-item" | "set-content" | "add-content" | "out-file" | "new-item"
        | "clear-content" => {
            paths.extend(operands());
        }
        "mv" | "move" | "move-item" | "rename-item" => paths.extend(operands()),
        "cp" | "copy" | "copy-item" | "install" | "ln" => paths.extend(operands().next_back()),
        "sed" | "perl" if inv.args.iter().any(|a| a.starts_with("-i") || a == "--in-place") => {
            paths.extend(operands().skip(1));
        }
        _ => {}
    }
    paths
}

fn is_discard(out: &str) -> bool {
    matches!(
        out.to_ascii_lowercase().as_str(),
        "/dev/null" | "nul" | "$null" | "1" | "2" | "/dev/stderr" | "/dev/stdout"
    )
}

fn is_read_only_segment(seg: &Segment) -> bool {
    let words: Vec<&String> =
        seg.args.iter().skip_while(|w| KEYWORDS.contains(&w.as_str())).collect();
    let Some((head, rest)) = words.split_first() else {
        return true;
    };
    let args: Vec<String> = rest.iter().map(|w| (*w).clone()).collect();
    match command_name(head).as_str() {
        "git" => is_read_only_git(&args),
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

fn is_read_only_git(args: &[String]) -> bool {
    let launches = args.iter().any(|a| {
        a == "-c"
            || a == "-O"
            || a == "--ext-diff"
            || a.starts_with("--output")
            || a.starts_with("--exec-path")
            || a.starts_with("--open-files-in-pager")
    });
    let Some((sub, rest)) = split_git_subcommand(args) else {
        return false;
    };
    let lists = |allowed: &[&str]| rest.iter().all(|a| allowed.contains(&a.as_str()));
    let reads = GIT_READERS.contains(&sub)
        || match sub {
            "branch" => lists(&["-a", "-r", "-v", "-vv", "--list", "--show-current", "--all"]),
            "remote" => lists(&["-v", "show", "origin"]),
            "tag" => lists(&["-l", "--list"]),
            "stash" => rest.first().is_some_and(|a| a == "list" || a == "show"),
            "worktree" => rest.first().is_some_and(|a| a == "list"),
            _ => false,
        };
    reads && !launches
}

fn is_read_only_anvil(args: &[String]) -> bool {
    let (sub, rest) = match anvil_subcommand(args) {
        AnvilArgs::Unparsed => return false,
        AnvilArgs::NoSub => return args.iter().any(|a| a == "--version" || a == "--help"),
        AnvilArgs::Sub(sub, rest) => (sub, rest),
    };
    match sub {
        "run" => {
            is_verification(&rest.iter().skip_while(|a| a.as_str() == "--").collect::<Vec<_>>())
        }
        "orchestrator" | "circuit" | "plugin" => rest.first().is_some_and(|a| a == "status"),
        "ratchet" => rest.first().is_some_and(|a| a != "promote"),
        "checkpoint" | "worktree" => rest.first().is_some_and(|a| a == "list"),
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
mod tests {
    use super::*;

    #[test]
    fn orchestrator_may_inspect_and_verify_only() {
        for ok in [
            "git status",
            "git diff HEAD~1 -- src | head -50",
            "git log --oneline | wc -l",
            "ls -la",
            "cat a.rs 2>/dev/null",
            "rg -n foo src && echo done",
            "sed -n '1,40p' a.rs",
            "cargo test -p x",
            "anvil check",
            "anvil run -- cargo clippy",
            "anvil diff",
            "npm test",
            "find . -name '*.rs'",
            "git branch --show-current",
            "echo $(git rev-parse HEAD)",
            "anvil orchestrator status",
            "for f in *.rs; do wc -l \"$f\"; done",
            "Get-Content a.rs | Select-Object -First 5",
        ] {
            assert_eq!(why_not_read_only(ok), None, "{ok}");
        }
        for bad in [
            "echo x > a.rs",
            "cat a >> b",
            "sed -i s/a/b/ a.rs",
            "sed -n 'w out' a",
            "tee a.rs",
            "cp a b",
            "python fix.py",
            "python -c 'open(\"a\",\"w\")'",
            "bash -c 'echo x > a'",
            "git commit -m x",
            "git checkout .",
            "git apply p",
            "git -c core.pager=sh log",
            "git diff --output=a.rs",
            "anvil orchestrator off",
            "anvil sync",
            "cargo clippy --fix",
            "npx eslint --fix src",
            "cargo fmt",
            "npm install",
            "node x.js",
            "find . -delete",
            "find . -exec rm {} +",
            "echo $(touch a.rs)",
            "ls; touch a",
            "anvil run -- touch a.rs",
            "Set-Content a.rs x",
            "sort -o a.rs b",
            "env touch a",
            "anvil ratchet promote x",
            "X=1 touch a",
            "xargs rm",
            "cat <<EOF > a.rs\nx\nEOF",
        ] {
            assert!(why_not_read_only(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn plugin_changes_are_for_the_user_and_status_is_readable() {
        assert_eq!(why_not_read_only("anvil plugin status --json"), None);
        for bad in ["anvil plugin install", "anvil plugin uninstall", "anvil plugin"] {
            assert!(why_not_read_only(bad).is_some(), "orchestrator: {bad}");
        }
        for bad in ["anvil plugin install", "anvil plugin uninstall", "anvil plugin"] {
            assert!(why_not_executor(bad).is_some(), "executor: {bad}");
        }
        assert_eq!(why_not_executor("anvil plugin status"), None);
        assert_eq!(why_not_executor("anvil plugin status --json"), None);
    }

    #[test]
    fn global_options_cannot_hide_a_reserved_anvil_subcommand() {
        let spellings = [
            "anvil --root . {}",
            "anvil --root=. {}",
            "anvil --no-color {}",
            "anvil --no-color --root . {}",
            "anvil --root . --no-color {}",
            "anvil.exe --root . {}",
            "./target/debug/anvil --root=x {}",
            "C:\\tools\\anvil.exe --no-color {}",
            "/usr/bin/anvil {}",
            "bash -c 'anvil --root . {}'",
            "sh -c \"anvil --root=. {}\"",
            "anvil --unknown {}",
            "anvil --unknown x {}",
        ];
        for tpl in spellings {
            for reserved in [
                "orchestrator off",
                "orchestrator on",
                "plugin install",
                "init",
                "sync",
                "circuit reset",
            ] {
                let cmd = tpl.replace("{}", reserved);
                assert!(why_not_executor(&cmd).is_some(), "executor: {cmd}");
                assert!(why_not_read_only(&cmd).is_some(), "orchestrator: {cmd}");
            }
            // The orchestrator owns task bookkeeping; only executors are kept away from it.
            let tasks = tpl.replace("{}", "tasks done T-1");
            assert!(why_not_executor(&tasks).is_some(), "executor: {tasks}");
        }
        for flags_after in [
            "anvil orchestrator off --root .",
            "anvil orchestrator --no-color off",
            "anvil --root . plugin --root . install",
        ] {
            assert!(why_not_executor(flags_after).is_some(), "{flags_after}");
            assert!(why_not_read_only(flags_after).is_some(), "{flags_after}");
        }
    }

    #[test]
    fn global_options_keep_allowed_anvil_commands_allowed() {
        for ok in [
            "anvil --root . check",
            "anvil --root=. check",
            "anvil --no-color --root . diff",
            "anvil.exe --root . plugin status",
        ] {
            assert_eq!(why_not_executor(ok), None, "executor: {ok}");
            assert_eq!(why_not_read_only(ok), None, "orchestrator: {ok}");
        }
        assert_eq!(why_not_read_only("anvil --version"), None);
        assert!(why_not_read_only("anvil --unknown check").is_some());
        assert_eq!(why_not_executor("anvil --unknown check"), None);
    }

    #[test]
    fn executor_cannot_take_orchestrator_decisions() {
        for bad in [
            "git commit -m x",
            "sudo git push",
            "bash -c 'git reset --hard'",
            "anvil tasks done T-1 x",
            "anvil orchestrator off",
            "anvil circuit reset",
        ] {
            assert!(why_not_executor(bad).is_some(), "{bad}");
        }
        for ok in
            ["git status", "cargo test", "anvil check", "anvil tasks list", "anvil run -- npm test"]
        {
            assert_eq!(why_not_executor(ok), None, "{ok}");
        }
        assert_eq!(
            written_paths("echo x > src/a.rs && sed -i s/a/b/ b.rs && cp c d"),
            vec!["src/a.rs", "b.rs", "d"]
        );
        assert_eq!(written_paths("bash -c 'echo x > in.rs'; cargo test 2>&1"), vec!["in.rs"]);
    }
}
