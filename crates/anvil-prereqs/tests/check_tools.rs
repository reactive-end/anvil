//! Tool checks against a scripted machine: nothing is spawned.

use anvil_core::config::prereqs::{CustomPrereq, PrereqSet, ToolPrereq};
use anvil_prereqs::{
    CheckOptions, HostOs, Project, Report, ScriptedHost, Status, check, projects_with,
    restrict_to_subrepo,
};
use tempfile::TempDir;

fn tool(name: &str, version: Option<&str>, required: bool) -> ToolPrereq {
    ToolPrereq {
        name: name.to_owned(),
        version: version.map(str::to_owned),
        source: "package.json engines.node".to_owned(),
        subrepo: None,
        required,
        hint: String::new(),
    }
}

fn run(host: &ScriptedHost, set: &PrereqSet) -> Report {
    let ws = TempDir::new().expect("tempdir");
    check(host, ws.path(), &projects_with(&[]), set, &CheckOptions::default())
}

fn single(report: &Report) -> &anvil_prereqs::Item {
    assert_eq!(report.items.len(), 1, "items: {:?}", report.items);
    &report.items[0]
}

#[test]
fn matching_version_is_ok_and_reports_what_was_found() {
    let host = ScriptedHost::new().tool("node", "--version", "v24.16.0\n");
    let set =
        PrereqSet { tools: vec![tool("node", Some(">=24.15.0"), true)], ..Default::default() };
    let report = run(&host, &set);
    let item = single(&report);
    assert_eq!(item.status, Status::Ok);
    assert_eq!(item.found.as_deref(), Some("24.16.0"));
    assert_eq!(item.constraint.as_deref(), Some(">=24.15.0"));
    assert_eq!(item.source, "package.json engines.node");
    assert!(report.ok);
}

#[test]
fn old_version_is_wrong_version_and_fails_when_required() {
    let host = ScriptedHost::new().tool("node", "--version", "v18.20.1\n");
    let set =
        PrereqSet { tools: vec![tool("node", Some(">=24.15.0"), true)], ..Default::default() };
    let report = run(&host, &set);
    assert_eq!(single(&report).status, Status::WrongVersion);
    assert!(single(&report).hint.as_deref().unwrap_or_default().contains("Node.js"));
    assert!(!report.ok);
    assert_eq!(report.failures(), 1);
}

#[test]
fn missing_tool_fails_when_required_and_warns_when_optional() {
    let set = PrereqSet {
        tools: vec![tool("terraform", None, true), tool("gh", None, false)],
        ..Default::default()
    };
    let report = run(&ScriptedHost::new(), &set);
    assert_eq!(report.items[0].status, Status::Missing);
    assert_eq!(report.items[1].status, Status::Warning);
    assert!(!report.ok);
    assert_eq!((report.summary.missing, report.summary.warning), (1, 1));
}

#[test]
fn versions_are_read_from_each_tools_own_output() {
    let host = ScriptedHost::new()
        .reply("java", "-version", 0, "", "openjdk version \"25.0.1\" 2025-10-21\nOpenJDK Runtime\n")
        .tool("rustc", "--version", "rustc 1.89.0 (29483883e 2025-08-04)\n")
        .tool("go", "version", "go version go1.22.3 windows/amd64\n")
        .tool("terraform", "version -json", "{\"terraform_version\":\"1.9.5\",\"platform\":\"linux_amd64\"}")
        .tool("docker", "version --format {{.Client.Version}}", "27.1.1\n")
        .tool("php", "-v", "PHP 8.3.6 (cli) (built: Jan 1 2025)\n")
        .tool("uv", "--version", "uv 0.4.18\n")
        .tool("sonar-scanner", "--version", "INFO: Scanner configuration file: /opt/5.0.0/conf\nINFO: SonarScanner CLI 6.2.1.4610\n")
        .tool("adb", "version", "Android Debug Bridge version 1.0.41\nVersion 35.0.2-12147458\n");
    let set = PrereqSet {
        tools: vec![
            tool("java", Some("25"), true),
            tool("rust", Some(">=1.89"), true),
            tool("go", Some(">=1.22"), true),
            tool("terraform", Some("~> 1.9.0"), true),
            tool("docker", None, true),
            tool("php", Some("^8.2"), true),
            tool("uv", None, true),
            tool("sonar-scanner", None, true),
            tool("adb", None, false),
        ],
        ..Default::default()
    };
    let report = run(&host, &set);
    let found: Vec<(&str, Option<&str>, Status)> =
        report.items.iter().map(|i| (i.name.as_str(), i.found.as_deref(), i.status)).collect();
    assert_eq!(
        found,
        [
            ("java", Some("25.0.1"), Status::Ok),
            ("rust", Some("1.89.0"), Status::Ok),
            ("go", Some("1.22.3"), Status::Ok),
            ("terraform", Some("1.9.5"), Status::Ok),
            ("docker", Some("27.1.1"), Status::Ok),
            ("php", Some("8.3.6"), Status::Ok),
            ("uv", Some("0.4.18"), Status::Ok),
            ("sonar-scanner", Some("6.2.1.4610"), Status::Ok),
            ("adb", Some("35.0.2"), Status::Ok),
        ]
    );
}

#[test]
fn java_8_reports_as_1_8_and_still_matches_major_8() {
    let host = ScriptedHost::new().reply("java", "-version", 0, "", "java version \"1.8.0_392\"\n");
    let set = PrereqSet { tools: vec![tool("java", Some("8"), true)], ..Default::default() };
    assert_eq!(single(&run(&host, &set)).status, Status::Ok);
}

#[test]
fn python_falls_back_to_the_py_launcher_on_windows_and_skips_the_store_stub() {
    let host = ScriptedHost::new()
        .with_os(HostOs::Windows)
        .reply(
            "python",
            "--version",
            9009,
            "",
            "Python was not found; run without arguments to install from the Microsoft Store\n",
        )
        .tool("py", "-3 --version", "Python 3.12.4\n");
    let set =
        PrereqSet { tools: vec![tool("python", Some(">=3.11,<3.13"), true)], ..Default::default() };
    let report = run(&host, &set);
    assert_eq!(single(&report).status, Status::Ok);
    assert_eq!(single(&report).found.as_deref(), Some("3.12.4"));
}

#[test]
fn terraform_on_windows_is_probed_inside_wsl_when_not_installed_natively() {
    let host = ScriptedHost::new().with_os(HostOs::Windows).tool(
        "wsl",
        "terraform version -json",
        "{\"terraform_version\":\"1.9.5\"}",
    );
    let set =
        PrereqSet { tools: vec![tool("terraform", Some("1.9.5"), true)], ..Default::default() };
    let report = run(&host, &set);
    assert_eq!(single(&report).status, Status::Ok);
    assert_eq!(single(&report).message.as_deref(), Some("via WSL"));
}

#[test]
fn installed_tool_with_unreadable_output_only_warns() {
    let host = ScriptedHost::new().tool("node", "--version", "weird banner\n");
    let set = PrereqSet { tools: vec![tool("node", Some(">=20"), true)], ..Default::default() };
    let report = run(&host, &set);
    assert_eq!(single(&report).status, Status::Warning);
    assert!(report.ok);
}

#[test]
fn unparsable_constraint_warns_instead_of_failing() {
    let host = ScriptedHost::new().tool("node", "--version", "v20.1.0\n");
    let set = PrereqSet { tools: vec![tool("node", Some("lts/iron"), true)], ..Default::default() };
    assert_eq!(single(&run(&host, &set)).status, Status::Warning);
}

#[test]
fn java_home_must_point_at_a_matching_jdk() {
    let host = ScriptedHost::new()
        .reply("java", "-version", 0, "", "openjdk version \"25.0.1\"\n")
        .reply("/opt/jdk17/bin/java", "-version", 0, "", "openjdk version \"17.0.9\"\n")
        .env("JAVA_HOME", "/opt/jdk17");
    let mut java = tool("java", Some("25"), true);
    java.source = "gradle/gradle-daemon-jvm.properties toolchainVersion".to_owned();
    let set = PrereqSet { tools: vec![java], ..Default::default() };
    let report = run(&host, &set);
    assert_eq!(report.items.len(), 2);
    assert_eq!(report.items[0].status, Status::Ok);
    assert_eq!(report.items[1].name, "JAVA_HOME");
    assert_eq!(report.items[1].status, Status::WrongVersion);
    assert_eq!(report.items[1].found.as_deref(), Some("17.0.9"));
}

#[test]
fn unset_java_home_warns_for_gradle_projects() {
    let host = ScriptedHost::new().reply("java", "-version", 0, "", "openjdk version \"25.0.1\"\n");
    let mut java = tool("java", Some("25"), true);
    java.source = "gradle/gradle-daemon-jvm.properties toolchainVersion".to_owned();
    let report = run(&host, &PrereqSet { tools: vec![java], ..Default::default() });
    assert_eq!(report.items[1].name, "JAVA_HOME");
    assert_eq!(report.items[1].status, Status::Warning);
    assert!(report.ok);
}

#[test]
fn ios_tools_are_skipped_off_macos_and_checked_on_macos() {
    let set = PrereqSet {
        tools: vec![tool("xcodebuild", None, true), tool("pod", None, true)],
        ..Default::default()
    };
    assert!(run(&ScriptedHost::new(), &set).items.is_empty());
    let mac = ScriptedHost::new().with_os(HostOs::MacOs).tool(
        "xcodebuild",
        "-version",
        "Xcode 15.2\nBuild version 15C500b\n",
    );
    let report = run(&mac, &set);
    assert_eq!(report.items[0].found.as_deref(), Some("15.2"));
    assert_eq!(report.items[1].status, Status::Missing);
}

#[test]
fn anvil_itself_is_always_found() {
    let set = PrereqSet { tools: vec![tool("anvil", None, true)], ..Default::default() };
    let report = run(&ScriptedHost::new(), &set);
    assert_eq!(single(&report).status, Status::Ok);
    assert_eq!(single(&report).found.as_deref(), Some(env!("CARGO_PKG_VERSION")));
}

#[test]
fn chrome_is_found_through_chrome_bin_or_path() {
    let set = PrereqSet { tools: vec![tool("chrome", None, false)], ..Default::default() };
    let on_path = ScriptedHost::new().on_path("chromium");
    assert_eq!(single(&run(&on_path, &set)).status, Status::Ok);
}

#[test]
fn custom_checks_use_the_exit_code() {
    let host = ScriptedHost::new().shell("pg_isready -p 5433", 0).shell("false", 1);
    let custom = |name: &str, command: &str, required: bool| CustomPrereq {
        name: name.to_owned(),
        command: command.to_owned(),
        hint: "start the db".to_owned(),
        subrepo: None,
        required,
    };
    let set = PrereqSet {
        custom: vec![
            custom("db up", "pg_isready -p 5433", true),
            custom("hard", "false", true),
            custom("soft", "false", false),
        ],
        ..Default::default()
    };
    let report = run(&host, &set);
    let statuses: Vec<Status> = report.items.iter().map(|i| i.status).collect();
    assert_eq!(statuses, [Status::Ok, Status::Missing, Status::Warning]);
    assert_eq!(report.items[1].message.as_deref(), Some("exited with status 1"));
}

#[test]
fn subrepo_restriction_keeps_workspace_and_named_items() {
    let ws = TempDir::new().expect("tempdir");
    let projects =
        vec![Project::root(), Project::subrepo("api", "api"), Project::subrepo("web", "web")];
    let mut api = tool("node", Some("20"), true);
    api.subrepo = Some("api".to_owned());
    let mut web = tool("node", Some("22"), true);
    web.subrepo = Some("web".to_owned());
    let set = PrereqSet { tools: vec![tool("git", None, true), api, web], ..Default::default() };

    let (kept_projects, kept) = restrict_to_subrepo(&projects, &set, "web").expect("known subrepo");
    let host = ScriptedHost::new().tool("git", "--version", "git version 2.43.0\n").tool(
        "node",
        "--version",
        "v22.1.0\n",
    );
    let report = check(&host, ws.path(), &kept_projects, &kept, &CheckOptions::default());

    let names: Vec<(&str, &str)> =
        report.items.iter().map(|i| (i.name.as_str(), i.group.as_str())).collect();
    assert_eq!(names, [("git", "(root)"), ("node", "web")]);
    assert!(restrict_to_subrepo(&projects, &set, "nope").is_err());
}

#[test]
fn report_serialises_with_stable_field_names() {
    let host = ScriptedHost::new().tool("node", "--version", "v18.0.0\n");
    let set = PrereqSet { tools: vec![tool("node", Some(">=20"), true)], ..Default::default() };
    let json = serde_json::to_value(run(&host, &set)).expect("json");
    assert_eq!(json["ok"], false);
    assert_eq!(json["summary"]["wrong_version"], 1);
    let item = &json["items"][0];
    assert_eq!(item["status"], "wrong_version");
    assert_eq!(item["kind"], "tool");
    assert_eq!(item["found"], "18.0.0");
    assert_eq!(item["constraint"], ">=20");
    assert_eq!(item["group"], "(root)");
}
