//! Rust, Go, Dart/Flutter and PHP manifests.

use std::sync::OnceLock;

use regex::Regex;

use super::Scan;

pub(super) fn run(scan: &mut Scan<'_>) {
    rust(scan);
    go(scan);
    dart(scan);
    php(scan);
}

fn rust(scan: &mut Scan<'_>) {
    let Some(text) = scan.read("Cargo.toml") else { return };
    let doc = text.parse::<toml::Table>().unwrap_or_default();
    let package = doc.get("package").and_then(|p| p.get("rust-version"));
    let workspace =
        doc.get("workspace").and_then(|w| w.get("package")).and_then(|p| p.get("rust-version"));
    let minimum = package.or(workspace).and_then(toml::Value::as_str).map(|v| format!(">={v}"));
    let member = scan.in_cargo_workspace();
    if minimum.is_none() && member {
        return;
    }
    let detail = if minimum.is_some() { "rust-version" } else { "" };
    let source = scan.src("Cargo.toml", detail);
    scan.tool("rust", minimum, source.clone(), true);
    if !member {
        scan.tool("cargo", None, source, true);
    }
}

fn go(scan: &mut Scan<'_>) {
    static GO: OnceLock<Regex> = OnceLock::new();
    let Some(text) = scan.read("go.mod") else { return };
    let re =
        GO.get_or_init(|| Regex::new(r"(?m)^go\s+(\d+\.\d+(?:\.\d+)?)").expect("static pattern"));
    let minimum = re.captures(&text).map(|c| format!(">={}", &c[1]));
    let source = scan.src("go.mod", if minimum.is_some() { "go" } else { "" });
    scan.tool("go", minimum, source, true);
}

/// `environment.sdk` is a Dart constraint; Flutter projects (`sdk: flutter`) need the Flutter SDK instead.
fn dart(scan: &mut Scan<'_>) {
    let Some(text) = scan.read("pubspec.yaml") else { return };
    let flutter = text.lines().any(|l| l.trim() == "sdk: flutter");
    let constraint = environment_value(&text, "sdk");
    if flutter {
        let source = scan.src("pubspec.yaml", "sdk: flutter");
        scan.tool("flutter", environment_value(&text, "flutter"), source, true);
    } else {
        let source =
            scan.src("pubspec.yaml", if constraint.is_some() { "environment.sdk" } else { "" });
        scan.tool("dart", constraint, source, true);
    }
}

/// A scalar under the top-level `environment:` map of a pubspec.
fn environment_value(pubspec: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in pubspec.lines() {
        if !line.starts_with([' ', '\t']) && !line.trim().is_empty() {
            inside = line.trim_end() == "environment:";
            continue;
        }
        if inside
            && let Some(rest) = line.trim().strip_prefix(key).and_then(|r| r.strip_prefix(':'))
        {
            let value = rest.trim().trim_matches(['"', '\'']);
            return (!value.is_empty()).then(|| value.to_owned());
        }
    }
    None
}

fn php(scan: &mut Scan<'_>) {
    let Some(text) = scan.read("composer.json") else { return };
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let version = json["require"]["php"].as_str().map(str::to_owned);
    let source = scan.src("composer.json", if version.is_some() { "require.php" } else { "" });
    scan.tool("php", version, source.clone(), true);
    scan.tool("composer", None, source, true);
}
