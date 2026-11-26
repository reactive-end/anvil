//! Line normalization, triviality, and path eligibility for the duplication check.

use crate::test_tampering::is_test_file;

const CODE_EXTENSIONS: [&str; 14] =
    ["rs", "ts", "tsx", "js", "jsx", "mjs", "php", "py", "kt", "java", "dart", "gd", "go", "cs"];
const IGNORED_SEGMENTS: [&str; 11] = [
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    "generated",
    "fixtures",
    "testdata",
    "migrations",
    ".harness",
    "__snapshots__",
];
const IMPORT_PREFIXES: [&str; 14] = [
    "use ",
    "pub use ",
    "import ",
    "from ",
    "export {",
    "export *",
    "mod ",
    "pub mod ",
    "extern crate",
    "#include",
    "using ",
    "package ",
    "namespace ",
    "@",
];
const SINGLE_TOKENS: [&str; 17] = [
    "Ok(())",
    "return",
    "return;",
    "break",
    "break;",
    "continue;",
    "None",
    "None,",
    "true",
    "false",
    "else",
    "else {",
    "try {",
    "finally {",
    "pass",
    "end",
    "default:",
];

/// A normalized, non-blank, non-comment source line with its 0-based index in the file.
pub(super) type NormLine = (usize, String);

fn is_skipped(trimmed: &str) -> bool {
    trimmed.is_empty()
        || trimmed.starts_with("//")
        || trimmed.starts_with('#')
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
        || matches!(trimmed, "{" | "}" | "};" | ");" | ")")
}

/// Whitespace-collapsed form of one line.
pub(super) fn collapse(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Drops blank lines, comments, attributes, and bare closing punctuation, and collapses whitespace.
pub(super) fn normalize(lines: &[&str]) -> Vec<NormLine> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !is_skipped(line.trim()))
        .map(|(idx, line)| (idx, collapse(line)))
        .collect()
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn mix(hash: u64, byte: u8) -> u64 {
    (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
}

/// Hash of a line's collapsed text, computed without allocating it.
fn line_hash(line: &str) -> u64 {
    let mut hash = FNV_OFFSET;
    for (n, token) in line.split_whitespace().enumerate() {
        if n > 0 {
            hash = mix(hash, b' ');
        }
        hash = token.bytes().fold(hash, mix);
    }
    hash
}

/// `(index, hash)` of every line `normalize` keeps.
pub(super) fn hashed_lines(lines: &[&str]) -> Vec<(usize, u64)> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !is_skipped(line.trim()))
        .map(|(idx, line)| (idx, line_hash(line)))
        .collect()
}

/// Hash of a window of consecutive kept lines, salted with the language family.
pub(super) fn window_hash(language: &str, window: &[(usize, u64)]) -> u64 {
    let seed = language.bytes().fold(FNV_OFFSET, mix);
    window.iter().flat_map(|(_, h)| h.to_le_bytes()).fold(seed, mix)
}

/// Files whose new lines are compared: code, not tests, not generated, vendored, or build output.
pub(super) fn is_checked_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    let code = lower.rsplit_once('.').is_some_and(|(_, ext)| CODE_EXTENSIONS.contains(&ext));
    let generated = [".min.", ".generated.", ".g.dart", ".freezed.", "_pb2.", ".d.ts"]
        .iter()
        .any(|marker| lower.contains(marker));
    let ignored_dir =
        path.split('/').rev().skip(1).any(|segment| IGNORED_SEGMENTS.contains(&segment));
    code && !generated && !ignored_dir && !is_test_file(path)
}

/// Files of one language family only match each other.
pub(super) fn language_key(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map_or("", |(_, ext)| ext);
    match ext {
        "ts" | "tsx" | "js" | "jsx" | "mjs" => "js",
        "rs" => "rs",
        "py" => "py",
        "php" => "php",
        "kt" | "java" => "jvm",
        "dart" => "dart",
        "gd" => "gd",
        "go" => "go",
        _ => "cs",
    }
}

/// Imports, annotations, one-word statements, and punctuation-only lines carry no logic.
pub(super) fn is_trivial(line: &str) -> bool {
    line.len() < 4
        || SINGLE_TOKENS.contains(&line)
        || IMPORT_PREFIXES.iter().any(|prefix| line.starts_with(prefix))
        || (line.starts_with("const ") && line.contains("require("))
        || line.chars().all(|c| c.is_whitespace() || ")}];,".contains(c))
}

/// A window is worth reporting when most of its lines are logic and it has some substance.
pub(super) fn is_substantial(window: &[NormLine]) -> bool {
    let logic: Vec<&String> = window.iter().map(|(_, l)| l).filter(|l| !is_trivial(l)).collect();
    let needed = (window.len() * 3).div_ceil(4);
    logic.len() >= needed && logic.iter().map(|l| l.len()).sum::<usize>() >= window.len() * 12
}

/// `true` when the 1-based `line` lies in one of the inclusive `ranges`.
pub(super) fn in_ranges(ranges: &[(u32, u32)], line: u32) -> bool {
    ranges.iter().any(|&(start, end)| (start..=end).contains(&line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_drops_comments_blank_lines_and_closers() {
        let lines = ["fn a() {", "", "    // note", "    let  x = 1;", "}", "#[test]"];
        let norm = normalize(&lines);
        assert_eq!(norm, vec![(0, "fn a() {".to_owned()), (3, "let x = 1;".to_owned())]);
    }

    #[test]
    fn trivial_lines_are_imports_single_tokens_and_punctuation() {
        for trivial in [
            "use std::fs;",
            "import x from 'y';",
            "Ok(())",
            "return;",
            "@Component({",
            "}),",
            "else",
        ] {
            assert!(is_trivial(trivial), "{trivial}");
        }
        assert!(!is_trivial("let total = items.iter().map(|i| i.cost).sum::<u32>();"));
    }

    #[test]
    fn paths_that_are_tests_generated_or_not_code_are_not_checked() {
        for skipped in [
            "src/a.test.ts",
            "tests/x.rs",
            "node_modules/p/i.js",
            "src/api.generated.ts",
            "README.md",
            "types/a.d.ts",
        ] {
            assert!(!is_checked_path(skipped), "{skipped}");
        }
        assert!(is_checked_path("src/commands/plugin.rs"));
    }
}
