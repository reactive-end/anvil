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
        "anvil search \"security guard\" --code",
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
        "git apply p",
        "git am p",
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
fn the_orchestrator_may_commit_and_publish() {
    for ok in [
        "git add -A && git commit -m x",
        "git push origin main",
        "git checkout -b topic",
        "git switch -c topic",
        "git remote add origin https://example.com/r.git",
        "git stash",
        "git -C sub commit -m x",
        "gh pr view 3",
        "gh pr create --fill",
        "gh run watch 9",
        "gh auth status",
    ] {
        assert_eq!(why_not_read_only(ok), None, "{ok}");
    }
}

#[test]
fn gh_is_limited_to_reading_and_the_orchestrators_pull_request_writes() {
    for bad in [
        "gh pr merge 3",
        "gh issue create",
        "gh repo delete o/r",
        "gh release create v1",
        "gh api -X POST repos/o/r/issues",
        "gh pr view 3 > out.txt",
    ] {
        assert!(why_not_read_only(bad).is_some(), "orchestrator: {bad}");
    }
}

#[test]
fn help_is_readable_for_both_roles_and_never_hides_a_command() {
    for ok in [
        "anvil sync --help",
        "anvil sync -h",
        "anvil orchestrator off --help",
        "anvil tasks done --help",
        "anvil hook --help",
        "anvil help sync",
        "anvil help orchestrator off",
        "anvil --root . plugin install --help",
        "./target/debug/anvil.exe circuit reset --help",
        "anvil run --help",
    ] {
        assert_eq!(why_not_read_only(ok), None, "orchestrator: {ok}");
        assert_eq!(why_not_executor(ok), None, "executor: {ok}");
    }
    for bad in ["anvil sync", "anvil --root --help sync", "anvil tasks done T-1 -- -h"] {
        assert!(why_not_executor(bad).is_some(), "executor: {bad}");
    }
    for bad in ["anvil sync", "anvil --root --help sync", "anvil run touch a --help"] {
        assert!(why_not_read_only(bad).is_some(), "orchestrator: {bad}");
    }
}

#[test]
fn built_binaries_are_read_like_the_installed_one() {
    let query = "\"aws --profile prod ec2 terminate-instances --instance-ids i-1\"";
    for exe in [
        "./target/debug/anvil.exe",
        "target/release/anvil",
        ".\\target\\debug\\anvil.exe",
        "C:\\path\\target\\debug\\anvil.exe",
        "\"C:\\path with space\\target\\debug\\anvil.exe\"",
        "anvil",
    ] {
        for tail in ["", " 2>&1", " | head -5", " --tool Bash"] {
            let cmd = format!("{exe} guard test {query}{tail}");
            assert_eq!(why_not_read_only(&cmd), None, "orchestrator: {cmd}");
            assert_eq!(why_not_executor(&cmd), None, "executor: {cmd}");
        }
        let prefixed = format!("cd /repo && {exe} guard test {query}");
        assert_eq!(why_not_read_only(&prefixed), None, "{prefixed}");
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
fn guard_changes_are_for_the_user_and_listing_or_testing_is_readable() {
    for ok in ["anvil guard list", "anvil guard test \"aws s3 rb s3://b\" --tool Bash"] {
        assert_eq!(why_not_read_only(ok), None, "orchestrator: {ok}");
        assert_eq!(why_not_executor(ok), None, "executor: {ok}");
    }
    for bad in [
        "anvil guard set cloud allow",
        "anvil guard add allow x",
        "anvil guard remove x",
        "anvil guard reset",
        "anvil --root . guard set cloud ask",
    ] {
        assert!(why_not_read_only(bad).is_some(), "orchestrator: {bad}");
        assert!(why_not_executor(bad).is_some(), "executor: {bad}");
    }
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
        "anvil review approve T-1 ok",
        "anvil orchestrator off",
        "anvil circuit reset",
    ] {
        assert!(why_not_executor(bad).is_some(), "{bad}");
    }
    for ok in [
        "git status",
        "cargo test",
        "anvil check",
        "anvil tasks list",
        "anvil review packet T-1",
        "anvil run -- npm test",
    ] {
        assert_eq!(why_not_executor(ok), None, "{ok}");
    }
}

#[test]
fn orchestrator_reads_architecture_but_cannot_set_it() {
    for ok in [
        "anvil architecture show",
        "anvil architecture show --json",
        "anvil architecture detect",
        "anvil architecture check --json",
        "anvil --root sandbox architecture check",
    ] {
        assert_eq!(why_not_read_only(ok), None, "{ok}");
    }
    for bad in [
        "anvil architecture set hexagonal",
        "anvil architecture set clean --subrepo api",
        "anvil architecture",
    ] {
        assert!(why_not_read_only(bad).is_some(), "{bad}");
    }
}

#[test]
fn executors_may_set_the_architecture() {
    for cmd in ["anvil architecture set hexagonal", "anvil architecture check"] {
        assert_eq!(why_not_executor(cmd), None, "{cmd}");
    }
}

#[test]
fn orchestrator_reads_prompt_improve_but_cannot_change_prompt_assist() {
    for ok in ["anvil prompt improve \"fix the login\"", "anvil prompt improve --json \"x\""] {
        assert_eq!(why_not_read_only(ok), None, "{ok}");
    }
    for bad in ["anvil prompt assist coach", "anvil prompt assist off", "anvil prompt"] {
        assert!(why_not_read_only(bad).is_some(), "{bad}");
    }
}
