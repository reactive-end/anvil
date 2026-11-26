//! Whether two delegated scopes can touch the same file.

use crate::ratchet::glob_matches;

/// Returns the first pair of entries, one from each scope, that may name the same file.
///
/// Conservative: a glob counts as everything under its literal directory, so `docs/*.md`
/// overlaps `docs/guide.md` and `docs/`, but not `src/`.
pub fn overlapping_entry(a: &[String], b: &[String]) -> Option<(String, String)> {
    a.iter()
        .flat_map(|left| b.iter().map(move |right| (left, right)))
        .find(|(left, right)| entries_overlap(left, right))
        .map(|(left, right)| (left.clone(), right.clone()))
}

fn entries_overlap(left: &str, right: &str) -> bool {
    let (left, right) = (super::normalize(left), super::normalize(right));
    match (left.contains('*'), right.contains('*')) {
        (false, false) => nested(&left, &right),
        (true, false) => glob_touches(&left, &right),
        (false, true) => glob_touches(&right, &left),
        (true, true) => nested(&glob_base(&left), &glob_base(&right)),
    }
}

fn glob_touches(glob: &str, literal: &str) -> bool {
    glob_matches(glob, literal) || nested(&glob_base(glob), literal)
}

/// `true` when one path is the other or lies inside it; the empty path is the repository root.
fn nested(a: &str, b: &str) -> bool {
    let inside = |outer: &str, inner: &str| {
        outer.is_empty() || inner == outer || inner.starts_with(&format!("{outer}/"))
    };
    inside(a, b) || inside(b, a)
}

/// The directory a glob is confined to: everything before the segment holding the first `*`.
fn glob_base(glob: &str) -> String {
    let literal = &glob[..glob.find('*').unwrap_or(glob.len())];
    literal.rfind('/').map_or_else(String::new, |cut| literal[..cut].to_owned())
}

#[cfg(test)]
mod tests {
    use super::overlapping_entry;

    fn scope(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| (*p).to_owned()).collect()
    }

    #[test]
    fn nested_and_equal_paths_overlap() {
        let auth = scope(&["src/auth", "tests/auth.rs"]);
        assert!(overlapping_entry(&auth, &scope(&["src/auth/token.rs"])).is_some());
        assert!(overlapping_entry(&auth, &scope(&["./tests/auth.rs"])).is_some());
        assert!(overlapping_entry(&scope(&["src"]), &scope(&["src/ui/a.rs"])).is_some());
    }

    #[test]
    fn siblings_do_not_overlap() {
        let auth = scope(&["src/auth", "tests/auth.rs"]);
        assert_eq!(
            overlapping_entry(&auth, &scope(&["src/ui", "src/authz.rs", "tests/ui.rs"])),
            None
        );
    }

    #[test]
    fn globs_cover_their_directory() {
        let docs = scope(&["docs/*.md"]);
        assert!(overlapping_entry(&docs, &scope(&["docs/guide.md"])).is_some());
        assert!(overlapping_entry(&docs, &scope(&["docs"])).is_some());
        assert!(overlapping_entry(&docs, &scope(&["docs/api/*.md"])).is_some());
        assert_eq!(overlapping_entry(&docs, &scope(&["src/lib.rs"])), None);
        assert!(overlapping_entry(&scope(&["*.toml"]), &scope(&["crates/a/Cargo.toml"])).is_some());
    }
}
