//! Pure-Rust analyzer for Godot 4 files (`.gd`, `.tscn`, `.tres`, `.gdshader`).

pub mod gdscript;
pub mod scene;

use std::path::Path;
use std::sync::OnceLock;

use anvil_core::config::AnvilConfig;
use anvil_core::traits::{Diagnostic, FileAstFacts, Language, Severity};
use regex::Regex;

use crate::rules::root_hygiene;

static GODOT_ALLOW_RE: OnceLock<Option<Regex>> = OnceLock::new();

/// Analyzes a Godot `.gd`, `.tscn`, `.tres`, or `.gdshader` file without tree-sitter.
pub fn analyze_godot(
    path: &Path,
    normalized_file: &str,
    source: &str,
    blake3_hash: String,
    language: Language,
    config: &AnvilConfig,
) -> FileAstFacts {
    let mut facts = match language {
        Language::GdScript => gdscript::analyze(normalized_file, source, blake3_hash, config),
        Language::GodotScene => scene::analyze(path, normalized_file, source, blake3_hash, config),
        Language::GdShader => FileAstFacts {
            path: normalized_file.to_owned(),
            blake3_hash,
            language: Language::GdShader,
            symbols: Vec::new(),
            edges: Vec::new(),
            diagnostics: Vec::new(),
        },
        _ => FileAstFacts {
            path: normalized_file.to_owned(),
            blake3_hash,
            language,
            symbols: Vec::new(),
            edges: Vec::new(),
            diagnostics: Vec::new(),
        },
    };

    if let Some(diag) = root_hygiene::check_root_hygiene(path, normalized_file, config) {
        facts.diagnostics.push(diag);
    }

    facts
}

pub(crate) fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

pub(crate) fn has_godot_allow(
    comments: &[Option<&str>],
    line_1based: u32,
    rule_id: &str,
    config: &AnvilConfig,
) -> bool {
    if !config.allowed_escapes.iter().any(|item| item == "anvil-allow") {
        return false;
    }

    let Some(re) = GODOT_ALLOW_RE
        .get_or_init(|| {
            Regex::new(r"anvil-allow\(\s*([A-Za-z0-9_:-]+)\s*\)\s*:\s*[^\r\n]*[A-Za-z0-9]").ok()
        })
        .as_ref()
    else {
        return false;
    };

    let candidates = [Some(line_1based), line_1based.checked_sub(1)];
    for cand_line in candidates.into_iter().flatten() {
        let Some(Some(comment)) = cand_line.checked_sub(1).and_then(|i| comments.get(i as usize))
        else {
            continue;
        };
        for caps in re.captures_iter(comment) {
            if let Some(rule_cap) = caps.get(1)
                && rule_matches(rule_cap.as_str(), rule_id)
            {
                return true;
            }
        }
    }

    false
}

fn rule_matches(allowed_raw: &str, expected_raw: &str) -> bool {
    let allowed = allowed_raw.trim().to_ascii_lowercase();
    let expected = expected_raw.trim().to_ascii_lowercase();
    if allowed == "all" || allowed == expected {
        return true;
    }
    if let Some(short) = expected.strip_prefix("no-")
        && allowed == short
    {
        return true;
    }
    matches!(expected.as_str(), "no-todo-comments" | "no-narrative-comments")
        && allowed == "comments"
}

pub(crate) fn push_diag(
    diagnostics: &mut Vec<Diagnostic>,
    rule: &str,
    severity: Severity,
    file: &str,
    pos: (u32, u32),
    message: String,
    suggestion: Option<&str>,
) {
    diagnostics.push(Diagnostic {
        rule: rule.to_owned(),
        severity,
        file: file.to_owned(),
        line: pos.0,
        column: pos.1.max(1),
        message,
        suggestion: suggestion.map(str::to_owned),
    });
}
