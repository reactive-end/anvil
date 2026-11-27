//! Infrastructure: Terraform, Docker Compose and the DB clients behind it, SonarQube, GitHub, cloud CLIs.

use std::sync::OnceLock;

use regex::Regex;

use super::Scan;
use crate::envfile::{env_keys, read_text};

const COMPOSE_FILES: &[&str] =
    &["docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml"];

/// Image name fragment, client tool, and the container port clients connect to.
const DATABASES: &[(&str, &str, u16)] = &[
    ("postgres", "psql", 5432),
    ("postgis", "psql", 5432),
    ("mysql", "mysql", 3306),
    ("mariadb", "mysql", 3306),
    ("mongo", "mongosh", 27017),
    ("redis", "redis-cli", 6379),
];

pub(super) fn run(scan: &mut Scan<'_>) {
    terraform(scan);
    docker(scan);
    sonar(scan);
    if scan.has(".github") {
        let source = scan.src(".github", "");
        scan.tool("gh", None, source, false);
    }
    aws_profiles(scan);
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("static pattern"))
}

fn terraform(scan: &mut Scan<'_>) {
    static REQUIRED: OnceLock<Regex> = OnceLock::new();
    static PROVIDER: OnceLock<Regex> = OnceLock::new();
    let files = scan.find_files(3, |name| name.ends_with(".tf"));
    let mut any = false;
    for file in &files {
        let Some(text) = read_text(file) else { continue };
        any = true;
        let shown = scan.rel_of(file);
        if let Some(c) = regex(&REQUIRED, r#"required_version\s*=\s*"([^"]+)""#).captures(&text) {
            scan.tool(
                "terraform",
                Some(c[1].trim().to_owned()),
                format!("{shown} required_version"),
                true,
            );
        }
        for c in regex(&PROVIDER, r#"provider\s+"(aws|google|azurerm)""#).captures_iter(&text) {
            let (tool, why) = match &c[1] {
                "aws" => ("aws", "provider aws"),
                "google" => ("gcloud", "provider google"),
                _ => ("az", "provider azurerm"),
            };
            scan.tool(tool, None, format!("{shown} {why}"), false);
        }
    }
    let pinned = ci_terraform_pin(scan);
    if let Some((version, source)) = pinned {
        scan.tool("terraform", Some(version), source, true);
    } else if any && !scan.out.tools.iter().any(|t| t.name == "terraform") {
        let source = scan.rel_of(&files[0]);
        scan.tool("terraform", None, source, true);
    }
}

/// The exact Terraform version CI installs (`hashicorp/setup-terraform` `terraform_version`).
fn ci_terraform_pin(scan: &Scan<'_>) -> Option<(String, String)> {
    static PIN: OnceLock<Regex> = OnceLock::new();
    let re = regex(&PIN, r#"terraform_version:\s*['"]?([0-9][^'"\s#]*)"#);
    let workflows = scan.dir.join(".github/workflows");
    let mut entries: Vec<_> =
        std::fs::read_dir(workflows).ok()?.flatten().map(|e| e.path()).collect();
    entries.sort();
    entries.iter().find_map(|path| {
        let text = read_text(path)?;
        let version = re.captures(&text)?[1].to_owned();
        let name = path.file_name()?.to_string_lossy().into_owned();
        Some((version, scan.src(&format!(".github/workflows/{name}"), "terraform_version")))
    })
}

fn docker(scan: &mut Scan<'_>) {
    let Some(file) = COMPOSE_FILES.iter().find(|f| scan.has(f)) else { return };
    let source = scan.src(file, "");
    scan.tool("docker", None, source.clone(), true);
    scan.tool("docker compose", None, source.clone(), true);
    let text = scan.read(file).unwrap_or_default().to_ascii_lowercase();
    let images: Vec<&str> = text.lines().filter_map(|l| l.trim().strip_prefix("image:")).collect();
    for (fragment, client, container_port) in DATABASES {
        if !images.iter().any(|image| image.contains(fragment)) {
            continue;
        }
        scan.tool(client, None, format!("{source} ({fragment} service)"), false);
        if *client != "psql" {
            continue;
        }
        if scan.out.custom.iter().any(|c| c.name == "postgres reachable") {
            continue;
        }
        let published = Regex::new(&format!(r"(\d+):{container_port}\b"))
            .ok()
            .and_then(|re| re.captures(&text).and_then(|c| c[1].parse().ok()))
            .unwrap_or(*container_port);
        let command = format!("pg_isready -h localhost -p {published}");
        scan.custom("postgres reachable", command, "Start it with `docker compose up -d`", false);
        scan.tool("pg_isready", None, source.clone(), false);
    }
}

/// `sonar-scanner` plus the env var names its scripts read: keys with `SONAR` in the env example.
fn sonar(scan: &mut Scan<'_>) {
    if !scan.has("sonar-project.properties") {
        return;
    }
    let source = scan.src("sonar-project.properties", "");
    scan.tool("sonar-scanner", None, source, true);
    let example = [".env.example", ".env.sample", ".env.template"]
        .into_iter()
        .find_map(|f| scan.read(f).map(|t| (f, t)));
    let declared: Vec<(String, String)> = example
        .iter()
        .flat_map(|(file, text)| {
            let origin = scan.src(file, "");
            env_keys(text)
                .into_iter()
                .filter(|k| k.contains("SONAR"))
                .map(move |k| (k, origin.clone()))
        })
        .collect();
    let hint = "Set it in your shell or the project's .env before running the scanner";
    if declared.is_empty() {
        let origin = scan.src("sonar-project.properties", "");
        scan.env_var("SONAR_TOKEN", origin, false, hint);
    }
    for (name, origin) in declared {
        scan.env_var(&name, origin, true, hint);
    }
}

/// `--profile name`, `AWS_PROFILE=name` in shell scripts and env examples.
fn aws_profiles(scan: &mut Scan<'_>) {
    static PROFILE: OnceLock<Regex> = OnceLock::new();
    let re = regex(&PROFILE, r#"(?:--profile[ =]|AWS_PROFILE=)["']?([A-Za-z0-9_.\-]+)"#);
    let files = scan.find_files(3, |n| {
        [".sh", ".ps1", ".example", ".sample"].iter().any(|ext| n.ends_with(ext))
    });
    let mut seen: Vec<(String, String)> = Vec::new();
    for file in &files {
        let Some(text) = read_text(file) else { continue };
        for c in re.captures_iter(&text) {
            if seen.len() < 5 && !seen.iter().any(|(p, _)| p == &c[1]) {
                seen.push((c[1].to_owned(), scan.rel_of(file)));
            }
        }
    }
    for (profile, origin) in seen {
        scan.tool("aws", None, origin, false);
        let command = format!("aws configure list --profile {profile}");
        let hint = format!("Configure it with `aws configure sso --profile {profile}` and log in");
        scan.custom(&format!("aws profile {profile}"), command, &hint, false);
    }
}
