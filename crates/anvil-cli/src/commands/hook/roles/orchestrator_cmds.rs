//! The `git` and `gh` operations that belong to the orchestrator: it stages, commits, and publishes
//! when the user asks, and executors never do. The security guard still asks before the destructive
//! forms (`reset --hard`, force pushes), so only the launch flags are refused here.

use crate::commands::hook::shell::destructive::split_git_subcommand;

/// Subcommands that move work between the tree, the history, and remotes without writing code.
const GIT_OPERATIONS: &[&str] = &[
    "add",
    "commit",
    "push",
    "pull",
    "fetch",
    "merge",
    "rebase",
    "branch",
    "switch",
    "checkout",
    "tag",
    "stash",
    "cherry-pick",
    "revert",
    "restore",
    "reset",
    "remote",
    "worktree",
];
/// `gh` groups whose `view`-like actions only read.
const GH_READ_GROUPS: &[&str] = &["pr", "issue", "repo", "run", "release", "workflow", "api"];
const GH_READ_ACTIONS: &[&str] = &["view", "list", "status", "diff", "checks", "watch"];

/// Whether `git <args>` is one of the orchestrator's own operations. `apply` and `am` write code
/// and stay out, as do options that make git run another program.
pub fn is_orchestrator_git(args: &[String], readers: &[&str]) -> bool {
    let Some((sub, rest)) = split_git_subcommand(args) else {
        return false;
    };
    let globals = &args[..args.len() - rest.len() - 1];
    let launches_program = globals.iter().any(|a| a == "-c" || a.starts_with("--config-env"))
        || args.iter().any(|a| is_launch_option(a))
        || (sub == "rebase" && rest.iter().any(|a| a.starts_with("-x")));
    !launches_program && (readers.contains(&sub) || GIT_OPERATIONS.contains(&sub))
}

fn is_launch_option(arg: &str) -> bool {
    arg == "-O"
        || arg == "--ext-diff"
        || arg.starts_with("--output")
        || arg.starts_with("--exec")
        || arg.starts_with("--open-files-in-pager")
        || arg.starts_with("--upload-pack")
        || arg.starts_with("--receive-pack")
        || arg.starts_with("ext::")
}

/// Whether `gh <args>` reads, or is one of the few writes the orchestrator owns: opening and
/// commenting on a pull request.
pub fn is_orchestrator_gh(args: &[String]) -> bool {
    let mut words: Vec<&str> = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "-R" | "--repo" | "--hostname" => {
                rest.next();
            }
            flag if flag.starts_with('-') => {}
            word => words.push(word),
        }
        if words.len() == 2 {
            break;
        }
    }
    matches!(words.as_slice(), ["auth", "status"] | ["pr", "create" | "comment"])
        || matches!(
            words.as_slice(),
            [group, action] if GH_READ_GROUPS.contains(group) && GH_READ_ACTIONS.contains(action)
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(cmd: &str) -> Vec<String> {
        cmd.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn the_orchestrators_git_operations_are_accepted() {
        for sub in GIT_OPERATIONS {
            assert!(is_orchestrator_git(&words(&format!("{sub} --quiet")), &[]), "{sub}");
        }
        for ok in [
            "add -A",
            "commit -m msg",
            "commit -c HEAD",
            "push origin main",
            "switch -c topic",
            "-C sub fetch --all",
            "remote add origin https://example.com/r.git",
            "worktree add ../w topic",
            "stash pop",
            "rebase --continue",
            "reset HEAD~1",
        ] {
            assert!(is_orchestrator_git(&words(ok), &[]), "{ok}");
        }
    }

    #[test]
    fn code_writers_and_program_launchers_stay_refused() {
        for bad in [
            "apply p",
            "am p",
            "-c core.pager=sh log",
            "-c core.editor=x commit",
            "rebase -x make",
            "rebase --exec make",
            "--exec-path=/x status",
            "fetch --upload-pack=sh",
            "push --receive-pack=sh",
            "remote add o ext::sh",
            "log --output=a.rs",
            "clean -f",
            "config alias.x !sh",
            "",
        ] {
            assert!(!is_orchestrator_git(&words(bad), &[]), "{bad}");
        }
    }

    #[test]
    fn gh_reads_and_the_orchestrators_own_pull_request_writes_are_accepted() {
        for ok in [
            "pr view 3",
            "pr list --state open",
            "issue view 4",
            "repo view",
            "run watch 12",
            "run list",
            "release view v1",
            "workflow list",
            "pr checks 3 --watch",
            "pr diff 3",
            "auth status",
            "pr create --fill",
            "pr comment 3 -b ok",
            "-R o/r pr view 3",
        ] {
            assert!(is_orchestrator_gh(&words(ok)), "{ok}");
        }
        for bad in [
            "pr merge 3",
            "pr close 3",
            "issue create",
            "issue comment 3",
            "repo delete o/r",
            "release create v1",
            "workflow run x",
            "auth login",
            "api -X POST repos/o/r/issues",
            "extension install x",
            "",
        ] {
            assert!(!is_orchestrator_gh(&words(bad)), "{bad}");
        }
    }
}
