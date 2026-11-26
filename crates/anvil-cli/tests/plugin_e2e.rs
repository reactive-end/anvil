//! End-to-end tests for `anvil plugin` against a fake `claude` CLI that records its arguments.
//! Nothing here touches the real `~/.claude` or `~/.anvil`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

/// Minimal stand-in for `claude plugin ...`: state lives in `FAKE_CLAUDE_STATE`, every call is logged.
const FAKE_CLAUDE: &str = r##"
use std::path::{Path, PathBuf};
use std::{env, fs, io::Write, process};

fn plugin_version(marketplace: &str) -> String {
    let body = fs::read_to_string(Path::new(marketplace).join("anvil-mode/.claude-plugin/plugin.json")).unwrap();
    let rest = &body[body.find("\"version\"").unwrap() + 9..];
    let start = rest.find('"').unwrap() + 1;
    rest[start..start + rest[start..].find('"').unwrap()].to_string()
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let state = PathBuf::from(env::var("FAKE_CLAUDE_STATE").unwrap());
    let mut log = fs::OpenOptions::new().create(true).append(true).open(state.join("calls.log")).unwrap();
    writeln!(log, "{}", args.join(" ")).unwrap();
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    if let (Ok(fail), Some(sub)) = (env::var("FAKE_CLAUDE_FAIL"), a.get(1)) {
        if fail == *sub {
            eprintln!("simulated failure");
            process::exit(1);
        }
    }
    let (market, version) = (state.join("marketplace"), state.join("version"));
    match a.as_slice() {
        ["plugin", "marketplace", "list", ..] => match fs::read_to_string(&market) {
            Ok(p) => println!("[{{\"name\":\"anvil\",\"source\":\"directory\",\"path\":\"{}\"}}]", p.replace('\\', "\\\\")),
            Err(_) => println!("[]"),
        },
        ["plugin", "marketplace", "add", p, ..] => fs::write(&market, p).unwrap(),
        ["plugin", "marketplace", "remove", ..] => drop(fs::remove_file(&market)),
        ["plugin", "list", ..] => match fs::read_to_string(&version) {
            Ok(v) => println!("[{{\"id\":\"anvil-mode@anvil\",\"version\":\"{v}\",\"scope\":\"user\"}}]"),
            Err(_) => println!("[]"),
        },
        ["plugin", "install" | "update", ..] => {
            fs::write(&version, plugin_version(&fs::read_to_string(&market).unwrap())).unwrap()
        }
        ["plugin", "uninstall", ..] => drop(fs::remove_file(&version)),
        _ => {
            eprintln!("unexpected call");
            process::exit(2);
        }
    }
}
"##;

struct Fixture {
    _tmp: TempDir,
    bin_dir: PathBuf,
    state: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let (bin_dir, state, home) =
            (tmp.path().join("bin"), tmp.path().join("state"), tmp.path().join("home"));
        fs::create_dir_all(&bin_dir).unwrap();
        fs::create_dir_all(&state).unwrap();
        let source = tmp.path().join("fake_claude.rs");
        fs::write(&source, FAKE_CLAUDE).unwrap();
        let exe = bin_dir.join(format!("claude{}", env::consts::EXE_SUFFIX));
        let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
        let built = Command::new(rustc)
            .arg("--edition=2021")
            .arg(&source)
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "fake claude failed to compile: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        Self { _tmp: tmp, bin_dir, state, home }
    }

    fn anvil(&self, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
        let mut paths = vec![self.bin_dir.clone()];
        paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
        Command::new(env!("CARGO_BIN_EXE_anvil"))
            .arg("plugin")
            .args(args)
            .env("PATH", env::join_paths(paths).unwrap())
            .env("ANVIL_HOME", &self.home)
            .env("FAKE_CLAUDE_STATE", &self.state)
            .envs(extra_env.iter().copied())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn folder(&self) -> PathBuf {
        self.home.join("claude-plugin")
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.state.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn seed_older_marketplace(&self, other: &Path) {
        fs::write(self.state.join("marketplace"), other.to_string_lossy().as_bytes()).unwrap();
        fs::write(self.state.join("version"), "0.0.1").unwrap();
    }

    fn status(&self) -> Value {
        let out = self.anvil(&["status", "--json"], &[]);
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap()
    }
}

fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

fn ok(out: &Output) -> String {
    assert!(out.status.success(), "failed: {}", text(out));
    text(out)
}

fn files_under(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                out.push(path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

#[test]
fn install_reinstall_and_uninstall_go_through_the_claude_cli() {
    let fx = Fixture::new();
    let first = ok(&fx.anvil(&["install"], &[]));
    assert!(
        first.contains("installed anvil-mode@anvil") && first.contains("Restart Claude Code"),
        "{first}"
    );

    let shipped = files_under(&fx.folder());
    assert!(shipped.contains(&"anvil-mode/hooks/register.tsx".to_owned()), "{shipped:?}");
    assert!(shipped.iter().all(|f| !f.contains(".test.") && !f.ends_with("kit.ts")), "{shipped:?}");

    let calls = fx.calls();
    let folder = fx.folder().to_string_lossy().into_owned();
    assert!(
        calls.iter().any(|c| c.starts_with("plugin marketplace add ") && c.contains(&folder)),
        "{calls:?}"
    );
    assert!(
        calls.contains(&"plugin install anvil-mode@anvil --scope user".to_owned()),
        "{calls:?}"
    );

    ok(&fx.anvil(&["install"], &[]));
    assert!(fx.calls().contains(&"plugin update anvil-mode@anvil --scope user".to_owned()));

    let status = fx.status();
    assert_eq!(status["installedVersion"], status["embeddedVersion"]);
    assert_eq!(status["marketplace"]["anvilFolder"], json!(true));

    let gone = ok(&fx.anvil(&["uninstall"], &[]));
    assert!(gone.contains("removed marketplace"), "{gone}");
    assert!(!fx.folder().exists(), "owned files and folder are removed");
    assert_eq!(fx.status()["installedVersion"], Value::Null);
}

#[test]
fn install_repoints_a_marketplace_registered_elsewhere_and_upgrades() {
    let fx = Fixture::new();
    let elsewhere = fx.home.join("repo-checkout");
    fx.seed_older_marketplace(&elsewhere);

    let out = ok(&fx.anvil(&["install"], &[]));
    assert!(out.contains("repointing"), "{out}");
    let status = fx.status();
    assert_eq!(status["installedVersion"], status["embeddedVersion"]);
    assert_eq!(status["marketplace"]["anvilFolder"], json!(true));
    let calls = fx.calls();
    assert!(calls.contains(&"plugin update anvil-mode@anvil --scope user".to_owned()), "{calls:?}");
    assert!(
        !calls.iter().any(|c| c.starts_with("plugin marketplace remove")),
        "in-place switch: {calls:?}"
    );
}

#[test]
fn failed_install_restores_the_previous_marketplace() {
    let fx = Fixture::new();
    let elsewhere = fx.home.join("repo-checkout");
    fx.seed_older_marketplace(&elsewhere);

    let out = fx.anvil(&["install"], &[("FAKE_CLAUDE_FAIL", "update")]);
    assert!(!out.status.success());
    assert!(text(&out).contains("restored the previous marketplace"), "{}", text(&out));
    let last_add =
        fx.calls().into_iter().rfind(|c| c.starts_with("plugin marketplace add ")).unwrap();
    assert!(last_add.contains(&*elsewhere.to_string_lossy()), "{last_add}");
}

#[test]
fn uninstall_leaves_a_marketplace_that_is_not_anvils() {
    let fx = Fixture::new();
    fx.seed_older_marketplace(&fx.home.join("repo-checkout"));
    let out = ok(&fx.anvil(&["uninstall"], &[]));
    assert!(out.contains("left in place"), "{out}");
    assert!(!fx.calls().iter().any(|c| c.starts_with("plugin marketplace remove")));
}

#[test]
fn install_refuses_a_folder_holding_foreign_content() {
    let fx = Fixture::new();
    fs::create_dir_all(fx.folder()).unwrap();
    fs::write(fx.folder().join("mine.txt"), "keep").unwrap();

    let out = fx.anvil(&["install"], &[]);
    assert!(!out.status.success());
    assert!(text(&out).contains("did not write"), "{}", text(&out));
    assert_eq!(files_under(&fx.folder()), vec!["mine.txt".to_owned()]);
    assert!(!fx.calls().iter().any(|c| c.starts_with("plugin marketplace add")));
}

#[test]
fn missing_claude_fails_with_one_line_and_writes_nothing() {
    let fx = Fixture::new();
    let empty = fx.home.join("empty-path");
    fs::create_dir_all(&empty).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_anvil"))
        .args(["plugin", "install"])
        .env("PATH", &empty)
        .env("ANVIL_HOME", &fx.home)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let message = text(&out);
    assert!(message.contains("`claude` CLI was not found"), "{message}");
    assert!(!fx.folder().exists());
}

#[test]
fn orchestrator_mode_denies_plugin_changes() {
    let repo = TempDir::new().unwrap();
    let root = repo.path();
    Command::new("git").arg("init").current_dir(root).output().unwrap();
    let anvil = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_anvil"))
            .arg("--root")
            .arg(root)
            .args(args)
            .output()
            .unwrap()
    };
    ok(&anvil(&["init"]));
    ok(&anvil(&["orchestrator", "on"]));

    for command in ["anvil plugin install", "anvil plugin uninstall"] {
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "session_id": "plugin-1",
            "cwd": root.to_string_lossy(),
            "tool_name": "Bash",
            "tool_input": { "command": command },
        });
        let mut child = Command::new(env!("CARGO_BIN_EXE_anvil"))
            .arg("--root")
            .arg(root)
            .args(["hook", "PreToolUse", "--adapter", "claude"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), payload.to_string().as_bytes())
            .unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            parsed["hookSpecificOutput"]["permissionDecision"],
            json!("deny"),
            "{command}: {out}"
        );
    }
}
