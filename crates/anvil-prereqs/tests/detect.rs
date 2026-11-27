//! Detection from project files, on temporary fixtures.

mod common;

use anvil_core::config::prereqs::{PrereqSet, ToolPrereq};
use anvil_prereqs::{Project, detect, projects_with};
use common::{workspace, write};

fn tool<'a>(set: &'a PrereqSet, name: &str) -> Vec<&'a ToolPrereq> {
    set.tools.iter().filter(|t| t.name == name).collect()
}

fn one<'a>(set: &'a PrereqSet, name: &str) -> &'a ToolPrereq {
    let found = tool(set, name);
    assert_eq!(found.len(), 1, "expected exactly one `{name}` in {:?}", set.tools);
    found[0]
}

#[test]
fn nestjs_backend_with_compose_sonar_and_env_example() {
    let ws = workspace();
    let root = ws.path();
    write(
        root,
        "package.json",
        r#"{"name":"orders","engines":{"node":">=24.15.0","npm":">=10"},
            "dependencies":{"@nestjs/core":"^11"},"devDependencies":{"karma":"^6"}}"#,
    );
    write(root, "package-lock.json", "{}");
    write(root, ".npmrc", "legacy-peer-deps=true\n");
    write(
        root,
        "docker-compose.yml",
        "services:\n  db:\n    image: postgres:16\n    ports:\n      - \"5433:5432\"\n",
    );
    write(root, "sonar-project.properties", "sonar.projectKey=orders\n");
    write(root, ".env.example", "DATABASE_URL=\nURL_SONAR=\nAPI_KEY_SONAR=\n");

    let set = detect(root, &projects_with(&[]));

    let node = one(&set, "node");
    assert_eq!(node.version.as_deref(), Some(">=24.15.0"));
    assert_eq!(node.source, "package.json engines.node");
    assert!(node.required && node.subrepo.is_none());
    let npm = one(&set, "npm");
    assert_eq!(npm.version.as_deref(), Some(">=10"));
    assert!(npm.hint.contains("legacy-peer-deps"), "npm hint: {}", npm.hint);
    for name in ["git", "anvil", "docker", "docker compose", "sonar-scanner", "chrome"] {
        one(&set, name);
    }
    assert!(!one(&set, "psql").required, "DB clients are optional");
    let reachable = set.custom.iter().find(|c| c.name == "postgres reachable").expect("pg check");
    assert_eq!(reachable.command, "pg_isready -h localhost -p 5433");
    assert!(!reachable.required);
    let env_vars: Vec<&str> = set.env_vars.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(env_vars, ["URL_SONAR", "API_KEY_SONAR"]);
    assert!(set.env_vars.iter().all(|v| v.required));
    assert_eq!(set.env_files.len(), 1);
    assert_eq!(
        (set.env_files[0].file.as_str(), set.env_files[0].example.as_str()),
        (".env", ".env.example")
    );
    assert!(set.paths.iter().any(|p| p.path == "node_modules" && !p.required));
}

#[test]
fn node_pins_come_from_every_version_file() {
    let ws = workspace();
    let root = ws.path();
    write(root, "package.json", r#"{"name":"x","packageManager":"pnpm@9.1.0+sha512.abc"}"#);
    write(root, ".nvmrc", "v20.11.0\n");
    write(root, ".tool-versions", "nodejs 20.11.0\npython 3.11.4 # app\njava temurin-17.0.1\n");
    write(root, "mise.toml", "[tools]\nterraform = \"1.9.5\"\nnode = [\"22\", \"20\"]\n");

    let set = detect(root, &projects_with(&[]));

    let node_versions: Vec<_> =
        tool(&set, "node").iter().filter_map(|t| t.version.clone()).collect();
    assert_eq!(node_versions, ["20.11.0", "22"]);
    assert_eq!(one(&set, "pnpm").version.as_deref(), Some("9.1.0"));
    assert_eq!(one(&set, "python").version.as_deref(), Some("3.11.4"));
    assert_eq!(one(&set, "java").version.as_deref(), Some("17.0.1"));
    assert_eq!(one(&set, "terraform").version.as_deref(), Some("1.9.5"));
}

#[test]
fn python_uv_project_needs_python_uv_and_a_venv() {
    let ws = workspace();
    let root = ws.path();
    write(
        root,
        "pyproject.toml",
        "[project]\nname = \"svc\"\nrequires-python = \">=3.11,<3.13\"\n",
    );
    write(root, "uv.lock", "version = 1\n");

    let set = detect(root, &projects_with(&[]));

    let python = one(&set, "python");
    assert_eq!(python.version.as_deref(), Some(">=3.11,<3.13"));
    assert_eq!(python.source, "pyproject.toml requires-python");
    one(&set, "uv");
    let venv = set.paths.iter().find(|p| p.path == ".venv").expect(".venv path");
    assert!(venv.hint.contains("uv sync"), "hint: {}", venv.hint);
}

#[test]
fn gradle_android_project_with_daemon_jvm_25() {
    let ws = workspace();
    let root = ws.path();
    write(root, "gradlew", "#!/bin/sh\n");
    write(root, "settings.gradle.kts", "rootProject.name = \"app\"\n");
    write(
        root,
        "build.gradle.kts",
        "plugins { id(\"com.android.application\") }\nkotlin { jvmToolchain(17) }\n",
    );
    write(
        root,
        "gradle/gradle-daemon-jvm.properties",
        "toolchainVersion=25\ntoolchainVendor=ADOPTIUM\n",
    );

    let set = detect(root, &projects_with(&[]));

    let java = one(&set, "java");
    assert_eq!(java.version.as_deref(), Some("25"));
    assert_eq!(java.source, "gradle/gradle-daemon-jvm.properties toolchainVersion");
    assert!(one(&set, "android-sdk").required);
    assert!(!one(&set, "adb").required);
}

#[test]
fn gradle_toolchain_declaration_sets_a_floor_without_daemon_file() {
    let ws = workspace();
    let root = ws.path();
    write(root, "build.gradle.kts", "kotlin { jvmToolchain(21) }\n");

    let set = detect(root, &projects_with(&[]));

    assert_eq!(one(&set, "java").version.as_deref(), Some("21+"));
}

#[test]
fn terraform_required_version_ci_pin_and_providers() {
    let ws = workspace();
    let root = ws.path();
    write(
        root,
        "infra/main.tf",
        "terraform {\n  required_version = \">= 1.5.0, < 2.0.0\"\n}\nprovider \"aws\" {\n  region = \"us-east-1\"\n}\n",
    );
    write(
        root,
        ".github/workflows/ci.yml",
        "steps:\n  - uses: hashicorp/setup-terraform@v3\n    with:\n      terraform_version: 1.9.5\n",
    );

    let set = detect(root, &projects_with(&[]));

    let terraform: Vec<_> =
        tool(&set, "terraform").iter().filter_map(|t| t.version.clone()).collect();
    assert_eq!(terraform, [">= 1.5.0, < 2.0.0", "1.9.5"]);
    assert_eq!(tool(&set, "terraform")[0].source, "infra/main.tf required_version");
    assert!(!one(&set, "aws").required);
    assert!(!one(&set, "gh").required, "`.github` implies the GitHub CLI, optionally");
}

#[test]
fn native_manifests_set_minimum_versions() {
    let ws = workspace();
    let root = ws.path();
    write(root, "Cargo.toml", "[package]\nname = \"x\"\nrust-version = \"1.89\"\n");
    write(root, "go.mod", "module x\n\ngo 1.22.3\n");
    write(root, "composer.json", r#"{"require":{"php":"^8.2"}}"#);
    write(root, "pubspec.yaml", "name: app\nenvironment:\n  sdk: \">=3.4.0 <4.0.0\"\n");

    let set = detect(root, &projects_with(&[]));

    assert_eq!(one(&set, "rust").version.as_deref(), Some(">=1.89"));
    assert_eq!(one(&set, "go").version.as_deref(), Some(">=1.22.3"));
    assert_eq!(one(&set, "php").version.as_deref(), Some("^8.2"));
    assert_eq!(one(&set, "dart").version.as_deref(), Some(">=3.4.0 <4.0.0"));
}

#[test]
fn mcp_servers_are_listed_and_imply_chrome() {
    let ws = workspace();
    let root = ws.path();
    write(
        root,
        ".mcp.json",
        r#"{"mcpServers":{"chrome-devtools":{"command":"npx","args":["-y","chrome-devtools-mcp@latest"]},
            "db":{"command":"npx","args":["-y","@modelcontextprotocol/server-postgres"]}}}"#,
    );

    let set = detect(root, &projects_with(&[]));

    assert_eq!(set.mcp_servers, ["chrome-devtools", "db"]);
    assert!(!one(&set, "chrome").required);
}

#[test]
fn monorepo_subrepos_get_their_own_entries() {
    let ws = workspace();
    let root = ws.path();
    write(root, "orders-api/package.json", r#"{"engines":{"node":"20"}}"#);
    write(root, "orders-api/.env.example", "A=\n");
    write(root, "web/package.json", r#"{"engines":{"node":">=22"}}"#);
    write(root, "web/pnpm-lock.yaml", "");
    let projects = vec![
        Project::root(),
        Project::subrepo("orders-api", "orders-api"),
        Project::subrepo("web", "web"),
    ];

    let set = detect(root, &projects);

    let nodes = tool(&set, "node");
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].subrepo.as_deref(), Some("orders-api"));
    assert_eq!(nodes[0].source, "orders-api/package.json engines.node");
    assert_eq!(nodes[1].subrepo.as_deref(), Some("web"));
    assert_eq!(nodes[1].version.as_deref(), Some(">=22"));
    assert_eq!(one(&set, "pnpm").subrepo.as_deref(), Some("web"));
    assert_eq!(one(&set, "npm").subrepo.as_deref(), Some("orders-api"));
    assert!(one(&set, "git").subrepo.is_none(), "workspace tools belong to the root");
    assert_eq!(set.env_files[0].file, "orders-api/.env");
    assert_eq!(set.env_files[0].subrepo.as_deref(), Some("orders-api"));
}

#[test]
fn cargo_workspace_members_do_not_repeat_the_workspace_toolchain() {
    let ws = workspace();
    let root = ws.path();
    write(root, "Cargo.toml", "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\n");
    write(root, "crates/a/Cargo.toml", "[package]\nname = \"a\"\n");
    write(root, "crates/b/Cargo.toml", "[package]\nname = \"b\"\nrust-version = \"1.90\"\n");
    let projects =
        vec![Project::root(), Project::subrepo("a", "crates/a"), Project::subrepo("b", "crates/b")];

    let set = detect(root, &projects);

    let rust: Vec<(Option<&str>, Option<&str>)> =
        tool(&set, "rust").iter().map(|t| (t.subrepo.as_deref(), t.version.as_deref())).collect();
    assert_eq!(rust, [(None, None), (Some("b"), Some(">=1.90"))]);
    assert_eq!(tool(&set, "cargo").len(), 1);
}
