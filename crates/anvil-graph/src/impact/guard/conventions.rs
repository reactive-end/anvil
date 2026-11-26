//! Files and symbols the graph cannot see being used: entry points, frameworks, macros, and tests.

use std::fs;
use std::path::Path;

use anvil_core::traits::Language;
use serde_json::Value;

use crate::impact::sibling::is_test_file;
use crate::model::GraphNode;

const SAFE_DECORATORS: [&str; 11] = [
    "override", "immutable", "dataclass", "property", "staticmethod", "classmethod", "abstractmethod",
    "deprecated", "final", "visibleForTesting", "protected",
];
const SAFE_RUST_ATTRIBUTES: [&str; 5] = ["doc", "allow", "must_use", "inline", "deprecated"];
const STD_DERIVES: [&str; 9] =
    ["Debug", "Clone", "Copy", "PartialEq", "Eq", "Hash", "PartialOrd", "Ord", "Default"];
const JS_ENTRY_STEMS: [&str; 18] = [
    "main", "server", "app", "worker", "entry", "cli", "bin", "preload", "polyfills", "setup", "setupTests",
    "global", "vite-env", "middleware", "instrumentation", "page", "layout", "route",
];
const PYTHON_ENTRIES: [&str; 11] = [
    "__main__.py", "__init__.py", "conftest.py", "setup.py", "manage.py", "wsgi.py", "asgi.py", "settings.py",
    "urls.py", "admin.py", "apps.py",
];
const IGNORED_SEGMENTS: [&str; 11] = [
    "node_modules", "dist", "build", "target", "vendor", "docs", "scripts", "migrations", "generated",
    ".storybook", "public",
];

/// Languages whose references the resolver follows; Kotlin, PHP, and Godot depend on package, autoload,
/// and scene conventions that cannot be checked here.
pub(super) fn is_checked_language(language: Language) -> bool {
    matches!(
        language,
        Language::Rust
            | Language::TypeScript
            | Language::Tsx
            | Language::JavaScript
            | Language::Jsx
            | Language::Python
            | Language::Dart
    )
}

fn segments(path: &str) -> Vec<&str> {
    path.split('/').collect()
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn stem(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}

/// `true` when any directory above the file is one of `names`.
fn has_segment(path: &str, names: &[&str]) -> bool {
    segments(path).iter().rev().skip(1).any(|s| names.contains(s))
}

/// File name without extensions or test affixes: `models.test.tsx`, `test_models.py`, `models_test.rs`.
pub(super) fn base_stem(path: &str) -> &str {
    let stem = stem(file_name(path));
    let stem = stem.strip_prefix("test_").unwrap_or(stem);
    stem.strip_suffix("_tests").or_else(|| stem.strip_suffix("_test")).unwrap_or(stem)
}

pub(super) fn is_test_like(path: &str) -> bool {
    is_test_file(path)
        || stem(file_name(path)) == "tests"
        || file_name(path) == "conftest.py"
        || has_segment(path, &["test", "tests", "__tests__", "__mocks__", "fixtures", "testdata", "e2e", "spec"])
}

/// Generated, vendored, build, or documentation paths.
pub(super) fn is_ignored_path(path: &str) -> bool {
    let name = file_name(path);
    path.starts_with(".harness/")
        || path.starts_with(".agents/")
        || path.starts_with(".claude/")
        || name.ends_with(".d.ts")
        || [".min.", ".generated.", ".g.dart", ".freezed.", "_pb2."].iter().any(|m| name.contains(m))
        || has_segment(path, &IGNORED_SEGMENTS)
}

/// Files that are started or loaded by something other than an import.
pub(super) fn is_entry_point(root: &Path, file: &str, language: Language) -> bool {
    match language {
        Language::Rust => is_rust_entry(file),
        Language::Python => PYTHON_ENTRIES.contains(&file_name(file)) || has_segment(file, &["bin"]),
        Language::Dart => is_dart_entry(file),
        _ => is_js_entry(root, file),
    }
}

fn is_rust_entry(file: &str) -> bool {
    matches!(file_name(file), "main.rs" | "lib.rs" | "build.rs")
        || has_segment(file, &["examples", "benches", "tests"])
        || file.starts_with("src/bin/")
        || file.contains("/src/bin/")
}

fn is_dart_entry(file: &str) -> bool {
    let parts = segments(file);
    let directly_in_lib = parts.len() >= 2 && parts[parts.len() - 2] == "lib";
    file_name(file) == "main.dart"
        || directly_in_lib
        || has_segment(file, &["bin", "tool", "example", "integration_test"])
}

fn is_js_entry(root: &Path, file: &str) -> bool {
    let name = file_name(file);
    let stem = stem(name);
    let dir = file.rsplit_once('/').map_or("", |(d, _)| d);
    let at_package_root = dir.is_empty() || dir == "src" || dir.ends_with("/src");
    name.contains(".config.")
        || name.contains(".stories.")
        || JS_ENTRY_STEMS.contains(&stem)
        || has_segment(file, &["pages", "routes", "bin", "cypress"])
        || (stem == "index" && at_package_root)
        || is_named_by_package_json(root, file)
}

/// `main`, `module`, `types`, `bin`, and `exports` of any `package.json` above `file`.
fn is_named_by_package_json(root: &Path, file: &str) -> bool {
    let mut dir = file;
    loop {
        dir = dir.rsplit_once('/').map_or("", |(d, _)| d);
        let raw = fs::read_to_string(root.join(dir).join("package.json")).ok();
        if let Some(json) = raw.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()) {
            let mut named = Vec::new();
            for key in ["main", "module", "types", "typings", "bin", "exports"] {
                collect_strings(json.get(key), &mut named);
            }
            if named.iter().any(|value| names_file(dir, value, file)) {
                return true;
            }
        }
        if dir.is_empty() {
            return false;
        }
    }
}

fn collect_strings<'a>(value: Option<&'a Value>, out: &mut Vec<&'a str>) {
    match value {
        Some(Value::String(s)) => out.push(s),
        Some(Value::Array(items)) => items.iter().for_each(|v| collect_strings(Some(v), out)),
        Some(Value::Object(map)) => map.values().for_each(|v| collect_strings(Some(v), out)),
        _ => {}
    }
}

fn names_file(dir: &str, value: &str, file: &str) -> bool {
    let rel = value.trim_start_matches("./");
    let full = if dir.is_empty() { rel.to_owned() } else { format!("{dir}/{rel}") };
    full == file || file.rsplit_once('.').is_some_and(|(without_ext, _)| full == without_ext)
}

/// Decorators and annotations register code with a framework; Python scripts run themselves.
pub(super) fn is_framework_managed(language: Language, source: &str) -> bool {
    match language {
        Language::Rust => false,
        Language::Python if source.contains("__main__") => true,
        _ => source.lines().any(|line| {
            line.trim_start()
                .strip_prefix('@')
                .is_some_and(|rest| rest.starts_with(char::is_alphabetic) && !is_safe_decorator(rest))
        }),
    }
}

fn is_safe_decorator(rest: &str) -> bool {
    let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    SAFE_DECORATORS.contains(&name.as_str())
}

/// Rust symbols carrying an attribute the graph cannot follow (`#[test]`, `#[no_mangle]`, `#[command]`,
/// derives of non-std traits, `cfg`, ...).
pub(super) fn is_macro_reached(language: Language, source: &str, node: &GraphNode) -> bool {
    if language != Language::Rust {
        return false;
    }
    let lines: Vec<&str> = source.lines().collect();
    let def_index = (node.line as usize).saturating_sub(1).min(lines.len());
    lines[..def_index]
        .iter()
        .rev()
        .map(|l| l.trim())
        .take_while(|l| l.starts_with("#[") || l.starts_with("//") || l.ends_with(")]") || *l == "]")
        .any(|l| !l.starts_with("//") && !is_safe_rust_attribute(l))
}

fn is_safe_rust_attribute(line: &str) -> bool {
    let Some(inner) = line.strip_prefix("#[") else {
        return false;
    };
    let name = inner.split(['(', '=', ']']).next().unwrap_or("").trim();
    if name == "derive" {
        return inner
            .trim_start_matches("derive(")
            .trim_end_matches(")]")
            .split(',')
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .all(|d| STD_DERIVES.contains(&d));
    }
    SAFE_RUST_ATTRIBUTES.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_entry_points_are_recognised() {
        for entry in ["src/main.rs", "crates/a/src/lib.rs", "build.rs", "src/bin/tool.rs", "examples/demo.rs"] {
            assert!(is_rust_entry(entry), "{entry}");
        }
        assert!(!is_rust_entry("src/commands/plugin.rs"));
    }

    #[test]
    fn rust_attributes_decide_what_the_graph_cannot_follow() {
        assert!(is_safe_rust_attribute("#[derive(Debug, Clone)]"));
        assert!(!is_safe_rust_attribute("#[derive(Debug, Parser)]"));
        assert!(!is_safe_rust_attribute("#[no_mangle]"));
        assert!(!is_safe_rust_attribute("#[test]"));
        assert!(is_safe_rust_attribute("#[allow(dead_code)]"));
    }

    #[test]
    fn decorators_mark_framework_code_except_plain_language_ones() {
        assert!(is_framework_managed(Language::TypeScript, "@Component({})\nexport class A {}"));
        assert!(!is_framework_managed(Language::Dart, "@override\nvoid f() {}"));
        assert!(is_framework_managed(Language::Python, "if __name__ == \"__main__\":\n    pass"));
    }
}
