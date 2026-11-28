//! Node.js projects: runtime, package manager, `node_modules`, and browsers for Karma or Playwright.

use serde_json::Value;

use super::Scan;
use super::pins::pin_version;

const BROWSER_PACKAGES: &[&str] =
    &["karma", "karma-chrome-launcher", "@playwright/test", "playwright"];

pub(super) fn run(scan: &mut Scan<'_>) {
    let pkg: Option<Value> = scan.read("package.json").and_then(|t| serde_json::from_str(&t).ok());
    let node_files = [".nvmrc", ".node-version"];
    if pkg.is_none() && !node_files.iter().any(|f| scan.has(f)) {
        return;
    }
    let engines = pkg.as_ref().and_then(|p| p.get("engines"));
    node_versions(scan, pkg.as_ref(), &node_files);
    package_manager(scan, pkg.as_ref(), engines);
    let Some(pkg) = pkg else { return };
    if has_dependencies(&pkg) {
        let hint = format!("Run `{}` in {}", install_command(scan), scan.project.rel_path);
        let source = scan.src("package.json", "dependencies");
        scan.path("node_modules", source, false, &hint);
    }
    if BROWSER_PACKAGES.iter().any(|name| dependency_names(&pkg).any(|d| d == *name)) {
        let source = scan.src("package.json", "karma/playwright");
        scan.tool("chrome", None, source, false);
    }
}

fn node_versions(scan: &mut Scan<'_>, pkg: Option<&Value>, node_files: &[&str]) {
    let engine = pkg.and_then(|p| p["engines"]["node"].as_str()).map(str::to_owned);
    let volta = pkg.and_then(|p| p["volta"]["node"].as_str()).map(str::to_owned);
    let mut found = false;
    if let Some(range) = engine {
        let source = scan.src("package.json", "engines.node");
        scan.tool("node", Some(range), source, true);
        found = true;
    }
    if let Some(version) = volta {
        let source = scan.src("package.json", "volta.node");
        scan.tool("node", Some(version), source, true);
        found = true;
    }
    for file in node_files {
        if let Some(version) = scan.read(file).as_deref().and_then(first_line).and_then(pin_version)
        {
            let source = scan.src(file, "");
            scan.tool("node", Some(version), source, true);
            found = true;
        }
    }
    if !found {
        let source = scan.src("package.json", "");
        scan.tool("node", None, source, true);
    }
}

fn first_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#'))
}

/// `packageManager` wins over the lockfile; `npm` is the default.
fn package_manager(scan: &mut Scan<'_>, pkg: Option<&Value>, engines: Option<&Value>) {
    let declared = pkg.and_then(|p| p["packageManager"].as_str());
    if let Some((name, version)) = declared.and_then(|d| d.split_once('@')) {
        let exact = version.split('+').next().unwrap_or(version).to_owned();
        let source = scan.src("package.json", "packageManager");
        scan.tool(name, Some(exact), source, true);
        return;
    }
    let name = manager_from_lockfile(scan);
    let range = engines.and_then(|e| e[name].as_str()).map(str::to_owned);
    let source = match &range {
        Some(_) => scan.src("package.json", &format!("engines.{name}")),
        None => scan.src(lockfile_of(name), ""),
    };
    if name == "npm" && scan.read(".npmrc").is_some_and(|t| t.contains("legacy-peer-deps")) {
        let hint = "This project sets legacy-peer-deps in .npmrc; install with `npm ci` and keep that file".to_owned();
        scan.tool_with_hint(name, range, scan.src(".npmrc", "legacy-peer-deps"), hint);
    } else {
        scan.tool(name, range, source, true);
    }
}

fn manager_from_lockfile(scan: &Scan<'_>) -> &'static str {
    if scan.has("pnpm-lock.yaml") {
        "pnpm"
    } else if scan.has("yarn.lock") {
        "yarn"
    } else if scan.has("bun.lockb") || scan.has("bun.lock") {
        "bun"
    } else {
        "npm"
    }
}

fn lockfile_of(manager: &str) -> &'static str {
    match manager {
        "pnpm" => "pnpm-lock.yaml",
        "yarn" => "yarn.lock",
        "bun" => "bun.lockb",
        _ => "package-lock.json",
    }
}

fn install_command(scan: &Scan<'_>) -> &'static str {
    match manager_from_lockfile(scan) {
        "pnpm" => "pnpm install --frozen-lockfile",
        "yarn" => "yarn install --frozen-lockfile",
        "bun" => "bun install --frozen-lockfile",
        _ => "npm ci",
    }
}

fn dependency_names(pkg: &Value) -> impl Iterator<Item = &str> {
    ["dependencies", "devDependencies"]
        .into_iter()
        .filter_map(|k| pkg.get(k).and_then(Value::as_object))
        .flat_map(|o| o.keys().map(String::as_str))
}

fn has_dependencies(pkg: &Value) -> bool {
    dependency_names(pkg).next().is_some()
}
