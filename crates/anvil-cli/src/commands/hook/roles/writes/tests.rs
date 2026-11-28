use std::path::Path;

use super::*;

fn paths(cmd: &str) -> Vec<String> {
    written_paths(cmd)
}

#[test]
fn redirections_and_file_writers_are_listed() {
    assert_eq!(
        paths("echo x > src/a.rs && sed -i s/a/b/ b.rs && cp c d"),
        vec!["src/a.rs", "b.rs", "d"]
    );
    assert_eq!(paths("bash -c 'echo x > in.rs'; cargo test 2>&1"), vec!["in.rs"]);
}

#[test]
fn powershell_parameter_values_are_not_paths() {
    let cases: [(&str, &[&str]); 12] = [
        ("New-Item -ItemType Directory -Path test/x", &["test/x"]),
        ("new-item -itemtype file -path:test/y -Force", &["test/y"]),
        ("New-Item test/z -ItemType Directory", &["test/z"]),
        ("Set-Content -Path a.txt -Value 'x y' -Encoding utf8", &["a.txt"]),
        ("Set-Content a.txt 'hello'", &["a.txt"]),
        ("echo hi | Out-File -Encoding utf8 -FilePath out.txt -Append", &["out.txt"]),
        ("Remove-Item -Recurse -Force test/x", &["test/x"]),
        ("ri -LiteralPath:test/q -Recurse", &["test/q"]),
        ("Copy-Item -Path a -Destination b -Recurse", &["b"]),
        ("Rename-Item -Path a.txt -NewName b.txt", &["a.txt"]),
        ("Rename-Item -Path a.txt -NewName sub/b.txt", &["a.txt", "sub/b.txt"]),
        ("Move-Item -Path a, b -Destination c", &["a", "b", "c"]),
    ];
    for (cmd, expected) in cases {
        assert_eq!(paths(cmd), expected, "{cmd}");
    }
}

#[test]
fn package_runners_do_not_turn_their_arguments_into_commands() {
    for cmd in [
        "npx playwright install chromium",
        "npx -y playwright install chromium",
        "npx --yes playwright@latest install --with-deps chromium",
        "pnpm dlx playwright install chromium",
        "yarn dlx playwright install chromium",
        "bunx playwright install chromium",
        "npx -p typescript tsc --init",
    ] {
        assert_eq!(paths(cmd), Vec::<String>::new(), "{cmd}");
    }
    assert_eq!(paths("npx -c 'touch a.txt'"), vec!["a.txt"]);
    assert_eq!(paths("npx rimraf dist"), Vec::<String>::new());
}

#[test]
fn install_needs_a_source_and_a_destination() {
    assert_eq!(paths("install chromium"), Vec::<String>::new());
    assert_eq!(paths("install -m 755 bin/tool /usr/local/bin/tool"), vec!["/usr/local/bin/tool"]);
    assert_eq!(paths("install -d out/a out/b"), vec!["out/a", "out/b"]);
    assert_eq!(paths("install -t out a b"), vec!["out"]);
}

#[test]
fn operands_with_unexpanded_variables_are_skipped() {
    assert_eq!(
        paths("touch $X ${Y}/a $env:Z/b %W%\\c ok.txt"),
        vec!["ok.txt"],
        "only literal operands can be checked"
    );
    assert_eq!(paths("echo x > $LOG"), Vec::<String>::new());
    assert_eq!(paths("echo 100% > pct.txt && touch 50%.txt"), vec!["pct.txt", "50%.txt"]);
}

#[test]
fn a_literal_cd_moves_the_base_of_later_commands() {
    assert_eq!(
        paths("cd test/hexagonal-sandbox/orders-api && cp ../README.md x"),
        vec!["test/hexagonal-sandbox/orders-api/x"]
    );
    assert_eq!(paths("cd a/b && touch ../c"), vec!["a/c"]);
    assert_eq!(paths("cd test; cd ../other; touch f"), vec!["other/f"]);
    assert_eq!(paths("pushd ..; touch a"), vec!["../a"]);
    assert_eq!(paths("Set-Location test/x; New-Item -ItemType File -Path f"), vec!["test/x/f"]);
    assert_eq!(paths("sl -Path test/x; touch f"), vec!["test/x/f"]);
    assert_eq!(paths("cd /abs && touch x ../y"), vec!["/abs/x", "/y"]);
    assert_eq!(paths("cd C:\\work\\p && touch ..\\q"), vec!["C:/work/q"]);
    assert_eq!(paths("cd -L a && touch f"), vec!["a/f"]);
}

#[test]
fn an_unknown_directory_leaves_relative_operands_unchecked() {
    for cmd in ["cd $X && touch a", "cd && touch a", "cd - && touch a", "cd ~ && touch a"] {
        assert_eq!(paths(cmd), Vec::<String>::new(), "{cmd}");
    }
    assert_eq!(paths("cd $X && touch /abs/a"), vec!["/abs/a"]);
    assert_eq!(paths("pushd a; popd; touch f"), Vec::<String>::new());
}

#[test]
fn a_cd_inside_a_nested_shell_does_not_leak_out() {
    assert_eq!(paths("bash -c 'cd /tmp; touch a'; touch b"), vec!["/tmp/a", "b"]);
    assert_eq!(paths("cd sub && bash -c 'touch ../a'"), vec!["a"]);
}

#[test]
fn lexical_cleaning_resolves_dots_without_the_filesystem() {
    assert_eq!(clean(Path::new("a/b/../c/./d")), Path::new("a/c/d"));
    assert_eq!(clean(Path::new("../x/../y")), Path::new("../y"));
    assert_eq!(clean(Path::new("a/../../x")), Path::new("../x"));
    assert_eq!(clean(Path::new("/a/../..")), Path::new("/"));
}
