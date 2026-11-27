//! Finds out whether a tool is installed and which version it reports.

use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;

use crate::host::{Host, HostOs, RunOutcome};

/// What probing a tool found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    NotFound,
    /// The tool ran but printed no version (or timed out).
    Unreadable(String),
    Found {
        version: String,
        note: Option<String>,
    },
}

#[derive(Clone, Copy)]
enum Parse {
    /// First `x.y[.z]` on stdout, then stderr.
    Generic,
    /// First capture of the regex, then `Generic`.
    Regex(&'static str),
    /// `version "17.0.9"` (stderr), with `1.8` read as 8.
    Java,
    /// A string field of the JSON printed on stdout.
    Json(&'static str),
}

struct Spec<'a> {
    program: &'a str,
    args: &'a [&'a str],
    parse: Parse,
    note: Option<&'static str>,
}

const fn spec<'a>(program: &'a str, args: &'a [&'a str], parse: Parse) -> Spec<'a> {
    Spec { program, args, parse, note: None }
}

/// Canonical tool name: aliases from `.tool-versions`, `mise.toml` and user config collapse to one.
pub fn canonical(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    match lower.as_str() {
        "nodejs" => "node",
        "golang" => "go",
        "python3" | "py" => "python",
        "rustc" | "rustup" => "rust",
        "jdk" | "openjdk" | "temurin" => "java",
        "aws-cli" | "awscli" => "aws",
        "docker-compose" => "docker compose",
        "sonar" | "sonarscanner" => "sonar-scanner",
        _ => return lower,
    }
    .to_owned()
}

fn specs(name: &str, os: HostOs) -> Vec<Spec<'_>> {
    use Parse::{Generic, Java, Json, Regex};
    match name {
        "python" if os == HostOs::Windows => {
            vec![spec("python", &["--version"], Generic), spec("py", &["-3", "--version"], Generic)]
        }
        "python" => {
            vec![spec("python3", &["--version"], Generic), spec("python", &["--version"], Generic)]
        }
        "java" => vec![spec("java", &["-version"], Java)],
        "rust" => vec![spec("rustc", &["--version"], Generic)],
        "go" => vec![spec("go", &["version"], Generic)],
        "terraform" => {
            let mut all = vec![spec("terraform", &["version", "-json"], Json("terraform_version"))];
            all.push(spec("terraform", &["version"], Generic));
            if os == HostOs::Windows {
                let mut wsl =
                    spec("wsl", &["terraform", "version", "-json"], Json("terraform_version"));
                wsl.note = Some("via WSL");
                all.push(wsl);
            }
            all
        }
        "docker" => vec![spec("docker", &["version", "--format", "{{.Client.Version}}"], Generic)],
        "docker compose" => vec![spec("docker", &["compose", "version", "--short"], Generic)],
        "flutter" => vec![spec("flutter", &["--version", "--machine"], Json("frameworkVersion"))],
        "php" => vec![spec("php", &["-v"], Generic)],
        "adb" => vec![spec("adb", &["version"], Regex(r"Version (\d+\.\d+\.\d+)"))],
        "sonar-scanner" => {
            vec![spec(
                "sonar-scanner",
                &["--version"],
                Regex(r"SonarScanner(?: CLI)? (\d+(?:\.\d+)+)"),
            )]
        }
        "xcodebuild" => vec![spec("xcodebuild", &["-version"], Generic)],
        "dart" => vec![spec("dart", &["--version"], Generic)],
        other => vec![Spec { program: other, args: &["--version"], parse: Generic, note: None }],
    }
}

/// Probes `name` (canonical) on `host`, trying each candidate command until one prints a version.
pub fn probe(host: &dyn Host, name: &str, timeout: Duration) -> Probe {
    let mut unreadable: Option<String> = None;
    for spec in specs(name, host.os()) {
        match attempt(host, spec.program, spec.args, spec.parse, timeout) {
            Attempt::Missing => {}
            Attempt::Unreadable(why) => unreadable = Some(why),
            Attempt::Version(version) => {
                return Probe::Found { version, note: spec.note.map(str::to_owned) };
            }
        }
    }
    unreadable.map_or(Probe::NotFound, Probe::Unreadable)
}

/// Probes the `java` under `JAVA_HOME`.
pub fn probe_java_home(host: &dyn Host, home: &str, timeout: Duration) -> Probe {
    let program = format!("{}/bin/java", home.trim_end_matches(['/', '\\']));
    match attempt(host, &program, &["-version"], Parse::Java, timeout) {
        Attempt::Missing => Probe::NotFound,
        Attempt::Unreadable(why) => Probe::Unreadable(why),
        Attempt::Version(version) => Probe::Found { version, note: None },
    }
}

enum Attempt {
    Missing,
    Unreadable(String),
    Version(String),
}

fn attempt(
    host: &dyn Host,
    program: &str,
    args: &[&str],
    how: Parse,
    timeout: Duration,
) -> Attempt {
    match host.run(program, args, timeout) {
        RunOutcome::NotFound => Attempt::Missing,
        RunOutcome::TimedOut => Attempt::Unreadable(format!("`{program}` timed out")),
        RunOutcome::Ran { code, stdout, stderr } => match parse(how, &stdout, &stderr) {
            Some(version) => Attempt::Version(version),
            None => Attempt::Unreadable(format!("`{program}` exited {code} without a version")),
        },
    }
}

fn parse(how: Parse, stdout: &str, stderr: &str) -> Option<String> {
    match how {
        Parse::Generic => generic(stdout).or_else(|| generic(stderr)),
        Parse::Regex(pattern) => {
            let re = Regex::new(pattern).ok()?;
            [stdout, stderr]
                .into_iter()
                .find_map(|text| re.captures(text).map(|c| c[1].to_owned()))
                .or_else(|| parse(Parse::Generic, stdout, stderr))
        }
        Parse::Java => java(stderr).or_else(|| java(stdout)),
        Parse::Json(field) => json_field(stdout, field).or_else(|| generic(stdout)),
    }
}

fn generic(text: &str) -> Option<String> {
    static VERSION: OnceLock<Regex> = OnceLock::new();
    let re = VERSION.get_or_init(|| Regex::new(r"\d+\.\d+(?:\.\d+)*").expect("static pattern"));
    re.find(text).map(|m| m.as_str().to_owned())
}

fn java(text: &str) -> Option<String> {
    static JAVA: OnceLock<Regex> = OnceLock::new();
    let re =
        JAVA.get_or_init(|| Regex::new(r#"version "(\d+(?:\.\d+)*)"#).expect("static pattern"));
    let raw = re.captures(text)?[1].to_owned();
    Some(raw.strip_prefix("1.").map_or(raw.clone(), str::to_owned))
}

fn json_field(text: &str, field: &str) -> Option<String> {
    let start = text.find('{')?;
    let value: serde_json::Value = serde_json::from_str(&text[start..]).ok()?;
    value.get(field)?.as_str().map(str::to_owned)
}
