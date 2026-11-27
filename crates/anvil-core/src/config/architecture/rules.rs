//! Import rules generated from an architecture preset.

use serde::{Deserialize, Serialize};

/// Prefix of every generated rule name (`arch:hexagonal:domain-no-framework`).
pub const RULE_PREFIX: &str = "arch:";

/// One generated import rule: a name, the reason that teaches the fix, and what it forbids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchRule {
    pub name: String,
    pub reason: String,
    pub kind: ArchRuleKind,
}

/// What a generated rule forbids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArchRuleKind {
    /// Files matching any `from` glob must not import files matching any `forbid` glob.
    Layer { from: Vec<String>, forbid: Vec<String> },
    /// Files matching any `from` glob must not import these packages (`@scope/*` or exact names).
    Packages { from: Vec<String>, packages: Vec<String> },
    /// Files of one module may only import another module through its public paths.
    Modules(ModuleIsolation),
}

/// Module isolation: modules are the directories directly under one of `roots`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModuleIsolation {
    /// Directories whose children are modules; the longest matching root wins.
    pub roots: Vec<String>,
    /// Module names that count as modules (empty: every directory).
    pub only: Vec<String>,
    /// Module names that may be imported freely (shared kernels).
    pub exempt: Vec<String>,
    /// Globs, relative to the imported module, that other modules may import.
    pub public: Vec<String>,
    /// When not empty, only files matching one of these globs are checked.
    pub from: Vec<String>,
}

impl ArchRule {
    /// One line summarizing what the rule forbids, for `anvil architecture show`.
    pub fn describe(&self) -> String {
        match &self.kind {
            ArchRuleKind::Layer { from, forbid } => {
                format!("{} must not import {}", from.join(" | "), forbid.join(" | "))
            }
            ArchRuleKind::Packages { from, packages } => {
                format!("{} must not import packages {}", from.join(" | "), packages.join(", "))
            }
            ArchRuleKind::Modules(iso) => {
                let sources =
                    if iso.from.is_empty() { "modules".to_owned() } else { iso.from.join(" | ") };
                format!(
                    "{sources} under {} may import another module only via {}",
                    iso.roots.join(" | "),
                    if iso.public.is_empty() {
                        "nothing".to_owned()
                    } else {
                        iso.public.join(" | ")
                    }
                )
            }
        }
    }
}

/// Builds the diagnostic message of a violating import, the format [`violation_target`] reads back.
pub fn violation_message(target: &str, rule: &str) -> String {
    format!("forbidden import of `{target}` (boundary `{rule}`)")
}

/// Extracts the imported target from a message made by [`violation_message`].
pub fn violation_target(message: &str) -> Option<&str> {
    let rest = message.strip_prefix("forbidden import of `")?;
    rest.split_once('`').map(|(target, _)| target)
}

/// Package name of an import specifier, or `None` for relative, absolute, and alias paths.
///
/// `typeorm/repository` -> `typeorm`; `@nestjs/common/decorators` -> `@nestjs/common`;
/// `node:fs` -> `fs`; `./x`, `../x`, `/x`, `@/x`, `~/x`, `#x` -> `None`.
pub fn package_name(specifier: &str) -> Option<&str> {
    let spec = specifier.trim().strip_prefix("node:").unwrap_or(specifier.trim());
    if spec.is_empty() || spec.starts_with(['.', '/', '~', '#', '\\']) || spec.starts_with("@/") {
        return None;
    }
    let mut parts = spec.splitn(3, '/');
    let first = parts.next()?;
    if first.starts_with('@') {
        let second = parts.next().filter(|s| !s.is_empty())?;
        return Some(&spec[..first.len() + 1 + second.len()]);
    }
    Some(first)
}

/// `true` when `package` matches `pattern`: an exact name, or `@scope/*` for any package of a scope.
pub fn package_matches(pattern: &str, package: &str) -> bool {
    match pattern.strip_suffix("/*") {
        Some(scope) => package.strip_prefix(scope).is_some_and(|rest| rest.starts_with('/')),
        None => pattern == package,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_ignore_subpaths_and_local_paths() {
        assert_eq!(package_name("typeorm/repository"), Some("typeorm"));
        assert_eq!(package_name("@nestjs/common"), Some("@nestjs/common"));
        assert_eq!(package_name("@nestjs/common/decorators"), Some("@nestjs/common"));
        assert_eq!(package_name("node:fs"), Some("fs"));
        for local in ["./a", "../a", "/a", "@/a", "~/a", "#a", "@scope"] {
            assert_eq!(package_name(local), None, "{local}");
        }
    }

    #[test]
    fn package_patterns_match_scopes_and_exact_names() {
        assert!(package_matches("@nestjs/*", "@nestjs/common"));
        assert!(!package_matches("@nestjs/*", "@nestjsx/common"));
        assert!(package_matches("typeorm", "typeorm"));
        assert!(!package_matches("pg", "pg-promise"));
        assert!(!package_matches("redis", "ioredis"));
    }

    #[test]
    fn violation_messages_round_trip_the_target() {
        let msg = violation_message("@nestjs/common", "arch:hexagonal:domain-no-framework");
        assert_eq!(violation_target(&msg), Some("@nestjs/common"));
        assert_eq!(violation_target("something else"), None);
    }
}
