//! Destructive-operation rules over expanded shell invocations.

use std::path::Path;

use super::command_name;
use super::expand::{Invocation, is_inert_text_command, string_literals};
use super::targets::is_dangerous;

const SQL_PATTERNS: &[(&str, &str)] = &[
    ("drop database", "destructive SQL (`DROP DATABASE`)"),
    ("drop schema", "destructive SQL (`DROP SCHEMA`)"),
    ("drop table", "destructive SQL (`DROP TABLE`)"),
    ("truncate table", "destructive SQL (`TRUNCATE TABLE`)"),
];
const SQL_CLIENTS: &[&str] = &[
    "psql", "mysql", "mariadb", "sqlite3", "sqlcmd", "sqlplus", "duckdb",
];
const FIND_FILTERS: &[&str] = &[
    "-name", "-iname", "-path", "-ipath", "-wholename", "-regex", "-iregex", "-empty", "-mtime",
    "-mmin", "-atime", "-ctime", "-newer", "-size", "-user",
];
const CODE_DELETE_MARKERS: &[&str] = &[
    "rmtree", "rmSync", "rmdirSync", "rm_rf", "rm_r(", "rimraf", "fs.rm(", "removeSync",
];

/// A deletion command split into its recursive flag and operands.
struct Deletion<'a> {
    recursive: bool,
    targets: Vec<&'a str>,
}

/// Describes why `inv` is destructive, or `None` when it is routine.
pub fn describe(root: &Path, inv: &Invocation) -> Option<&'static str> {
    if inv.name.is_empty() || is_inert_text_command(&inv.name, &inv.args) {
        return None;
    }
    match inv.name.as_str() {
        "git" => describe_git(&inv.args),
        "find" => describe_find(root, &inv.args),
        _ => describe_deletion(root, inv),
    }
    .or_else(|| describe_disk(inv))
    .or_else(|| describe_database(inv))
}

/// Describes destructive filesystem calls inside inline interpreter source.
pub fn describe_code(root: &Path, code: &str) -> Option<&'static str> {
    let deletes = CODE_DELETE_MARKERS
        .iter()
        .any(|marker| code.contains(marker));
    (deletes && string_literals(code).iter().any(|lit| is_dangerous(root, lit)))
        .then_some("recursive deletion from inline code")
}

/// Returns `true` when a git invocation disables hooks (`--no-verify`, `commit -n`, `core.hooksPath`).
pub fn bypasses_git_hooks(inv: &Invocation) -> bool {
    if inv.name != "git" {
        return false;
    }
    if inv
        .args
        .windows(2)
        .any(|w| w[0] == "-c" && w[1].to_ascii_lowercase().starts_with("core.hookspath="))
    {
        return true;
    }
    let Some((sub, rest)) = split_git_subcommand(&inv.args) else {
        return false;
    };
    let mut skip_value = false;
    for arg in rest {
        if std::mem::take(&mut skip_value) {
            continue;
        }
        if arg == "--" {
            return false;
        }
        if arg == "--no-verify" || (sub == "commit" && is_commit_skip_cluster(arg)) {
            return true;
        }
        skip_value = matches!(arg.as_str(), "--message" | "--file" | "--author" | "--date")
            || (is_short_cluster(arg) && arg.ends_with(['m', 'F', 'C', 'c', 't']));
    }
    false
}

/// Splits a git argument list into its subcommand and the arguments after it.
pub fn split_git_subcommand(args: &[String]) -> Option<(&str, &[String])> {
    let mut idx = 0usize;
    while let Some(arg) = args.get(idx) {
        if matches!(
            arg.as_str(),
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace"
        ) {
            idx += 2;
        } else if arg.starts_with('-') {
            idx += 1;
        } else {
            return Some((arg.as_str(), &args[idx + 1..]));
        }
    }
    None
}

fn is_commit_skip_cluster(arg: &str) -> bool {
    let value_flag = arg.find(['m', 'F', 'C', 'c', 't']).unwrap_or(arg.len());
    is_short_cluster(arg) && arg[..value_flag].contains('n')
}

fn is_short_cluster(arg: &str) -> bool {
    arg.len() > 1 && arg.starts_with('-') && arg[1..].chars().all(|c| c.is_ascii_alphabetic())
}

fn describe_git(args: &[String]) -> Option<&'static str> {
    let (sub, rest) = split_git_subcommand(args)?;
    let has = |flag: &str| rest.iter().any(|a| a == flag);
    let whole_tree = rest
        .iter()
        .any(|a| matches!(a.as_str(), "." | "./" | ":/" | "*" | ":"));
    match sub {
        "reset" if has("--hard") => Some("destructive git reset (`git reset --hard`)"),
        "checkout" if whole_tree || has_flag(rest, 'f', "--force") => {
            Some("discarding working tree changes (`git checkout`)")
        }
        "switch" if has("--discard-changes") || has_flag(rest, 'f', "--force") => {
            Some("discarding working tree changes (`git switch`)")
        }
        "restore" if whole_tree && restores_worktree(rest) => {
            Some("discarding all working tree changes (`git restore`)")
        }
        "clean" if has_flag(rest, 'f', "--force") && !has_flag(rest, 'n', "--dry-run") => {
            Some("force-deleting untracked files (`git clean -f`)")
        }
        "push" => describe_git_push(rest),
        "stash" if has("drop") || has("clear") => Some("deleting stashed work (`git stash`)"),
        "branch" if has_flag(rest, 'D', "-D") => Some("force-deleting a branch (`git branch -D`)"),
        "reflog" if has("expire") || has("delete") => Some("deleting git recovery history"),
        "filter-branch" | "filter-repo" => Some("rewriting repository history"),
        "update-ref" if has("-d") => Some("deleting a git ref"),
        _ => None,
    }
}

fn describe_git_push(rest: &[String]) -> Option<&'static str> {
    let forced = has_flag(rest, 'f', "--force")
        || rest.iter().any(|a| a == "--mirror" || a.starts_with('+'));
    if forced {
        return Some("force pushing (`git push --force`)");
    }
    let deletes = has_flag(rest, 'd', "--delete")
        || rest.iter().any(|a| a.len() > 1 && a.starts_with(':'));
    deletes.then_some("deleting a remote ref (`git push --delete`)")
}

fn restores_worktree(rest: &[String]) -> bool {
    !has_flag(rest, 'S', "--staged") || has_flag(rest, 'W', "--worktree")
}

/// Matches `--long` or a short flag inside a cluster such as `-fd`.
fn has_flag(args: &[String], short: char, long: &str) -> bool {
    args.iter()
        .any(|a| a == long || (is_short_cluster(a) && a.contains(short)))
}

fn describe_deletion(root: &Path, inv: &Invocation) -> Option<&'static str> {
    if inv.name.contains("io.directory]::delete") {
        return Some("recursive directory deletion through .NET");
    }
    if inv.args.iter().any(|a| a == "--no-preserve-root") {
        return Some("recursive deletion near filesystem root (`rm -rf /`)");
    }
    let del = parse_deletion(inv)?;
    if !del.recursive {
        return None;
    }
    if del.targets.is_empty() && inv.fed_by_pipe {
        return Some("bulk recursive deletion of piped paths");
    }
    del.targets.iter().any(|t| is_dangerous(root, t)).then_some(
        "recursive deletion of the root, home, working tree, `.git`, or a path outside the repository",
    )
}

fn parse_deletion(inv: &Invocation) -> Option<Deletion<'_>> {
    let is_windows_delete = matches!(inv.name.as_str(), "rd" | "rmdir" | "del" | "erase");
    if !is_windows_delete && !matches!(inv.name.as_str(), "rm" | "remove-item" | "ri" | "rimraf") {
        return None;
    }
    let mut del = Deletion {
        recursive: inv.name == "rimraf",
        targets: Vec::new(),
    };
    let mut flags_done = false;
    for arg in &inv.args {
        let lower = arg.to_ascii_lowercase();
        if flags_done {
            del.targets.push(arg);
        } else if lower == "--" {
            flags_done = true;
        } else if lower.starts_with('-') && lower.len() > 1 {
            del.recursive |= is_recursive_flag(&lower);
        } else if is_windows_delete && lower.len() == 2 && lower.starts_with('/') {
            del.recursive |= lower == "/s";
        } else if lower.contains('{') {
            del.targets.push(arg);
        } else {
            del.targets
                .extend(arg.split(',').map(str::trim).filter(|t| !t.is_empty()));
        }
    }
    Some(del)
}

fn is_recursive_flag(lower: &str) -> bool {
    if lower == "--recursive" || lower.starts_with("-rec") {
        return true;
    }
    !lower.starts_with("--")
        && lower.contains('r')
        && lower[1..]
            .chars()
            .all(|c| matches!(c, 'r' | 'f' | 'i' | 'v' | 'd'))
}

fn describe_find(root: &Path, args: &[String]) -> Option<&'static str> {
    let runs_rm = args.windows(2).any(|w| {
        matches!(w[0].as_str(), "-exec" | "-execdir" | "-ok") && command_name(&w[1]) == "rm"
    });
    if !runs_rm && !args.iter().any(|a| a == "-delete") {
        return None;
    }
    let filtered = args.iter().any(|a| FIND_FILTERS.contains(&a.as_str()));
    let outside = args
        .iter()
        .take_while(|a| !a.starts_with('-') && !matches!(a.as_str(), "!" | "("))
        .any(|start| !matches!(start.as_str(), "." | "./") && is_dangerous(root, start));
    (!filtered || outside).then_some("bulk deletion through `find`")
}

fn describe_disk(inv: &Invocation) -> Option<&'static str> {
    let name = inv.name.as_str();
    let raw_device_write = name == "dd" && inv.args.iter().any(|a| a.starts_with("of=/dev/"));
    let formats = name.starts_with("mkfs")
        || matches!(name, "diskpart" | "format-volume" | "clear-disk" | "wipefs");
    (raw_device_write || formats).then_some("formatting or overwriting a disk device")
}

fn describe_database(inv: &Invocation) -> Option<&'static str> {
    let has = |word: &str| inv.args.iter().any(|a| a.eq_ignore_ascii_case(word));
    if has("migrate:fresh") || has("db:wipe") {
        return Some("destructive database wipe (`migrate:fresh` / `db:wipe`)");
    }
    if (inv.name == "prisma" || has("prisma")) && has("migrate") && has("reset") {
        return Some("destructive database reset (`prisma migrate reset`)");
    }
    let mut sql = inv.args.join(" ");
    if SQL_CLIENTS.contains(&inv.name.as_str()) {
        for text in &inv.stdin_text {
            sql.push(' ');
            sql.push_str(text);
        }
    }
    let lower = sql.to_ascii_lowercase();
    let sql = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    SQL_PATTERNS
        .iter()
        .find(|(pattern, _)| sql.contains(pattern))
        .map(|(_, desc)| *desc)
}
