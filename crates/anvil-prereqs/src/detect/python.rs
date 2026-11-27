//! Python projects: interpreter version, uv or Poetry, and the `.venv`.

use super::Scan;
use super::pins::pin_version;

const MARKERS: &[&str] =
    &["pyproject.toml", "requirements.txt", "setup.py", "Pipfile", ".python-version"];

pub(super) fn run(scan: &mut Scan<'_>) {
    if !MARKERS.iter().any(|m| scan.has(m)) {
        return;
    }
    let pyproject = scan.read("pyproject.toml").and_then(|t| t.parse::<toml::Table>().ok());
    let mut versioned = false;
    if let Some(spec) = pyproject.as_ref().and_then(requires_python) {
        let source = scan.src("pyproject.toml", "requires-python");
        scan.tool("python", Some(spec), source, true);
        versioned = true;
    }
    if let Some(version) =
        scan.read(".python-version").as_deref().and_then(first_word).and_then(pin_version)
    {
        let source = scan.src(".python-version", "");
        scan.tool("python", Some(version), source, true);
        versioned = true;
    }
    if let Some(version) = pipfile_python(scan) {
        let source = scan.src("Pipfile", "requires.python_version");
        scan.tool("python", Some(version), source, true);
        versioned = true;
    }
    if !versioned {
        let source = scan.src(first_marker(scan), "");
        scan.tool("python", None, source, true);
    }
    let manager = installer(scan, pyproject.as_ref());
    if let Some((tool, lock)) = manager {
        let source = scan.src(lock, "");
        scan.tool(tool, None, source, true);
    }
    let fix = if matches!(manager, Some(("uv", _))) { "uv sync" } else { "python -m venv .venv" };
    let source = scan.src(first_marker(scan), "");
    scan.path(".venv", source, false, &format!("Create the virtual environment: `{fix}`"));
}

fn first_word(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#'))
}

fn first_marker(scan: &Scan<'_>) -> &'static str {
    MARKERS.iter().find(|m| scan.has(m)).copied().unwrap_or("pyproject.toml")
}

/// `project.requires-python` (PEP 621) or Poetry's `tool.poetry.dependencies.python`.
fn requires_python(doc: &toml::Table) -> Option<String> {
    let pep621 =
        doc.get("project").and_then(|p| p.get("requires-python")).and_then(toml::Value::as_str);
    let poetry = doc
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|p| p.get("dependencies"))
        .and_then(|d| d.get("python"))
        .and_then(toml::Value::as_str);
    pep621.or(poetry).map(str::to_owned)
}

fn pipfile_python(scan: &Scan<'_>) -> Option<String> {
    let doc = scan.read("Pipfile")?.parse::<toml::Table>().ok()?;
    let version = doc.get("requires")?.get("python_version")?.as_str()?;
    Some(version.to_owned())
}

/// uv or Poetry, from their lockfiles or config tables.
fn installer(
    scan: &Scan<'_>,
    pyproject: Option<&toml::Table>,
) -> Option<(&'static str, &'static str)> {
    let has_table =
        |name: &str| pyproject.and_then(|d| d.get("tool")).is_some_and(|t| t.get(name).is_some());
    if scan.has("uv.lock") || has_table("uv") {
        Some(("uv", if scan.has("uv.lock") { "uv.lock" } else { "pyproject.toml" }))
    } else if scan.has("poetry.lock") || has_table("poetry") {
        Some(("poetry", if scan.has("poetry.lock") { "poetry.lock" } else { "pyproject.toml" }))
    } else {
        None
    }
}
