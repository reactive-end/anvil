//! Test-file classification and directory-scoped sibling test discovery.

use crate::impact::ImpactedItem;
use crate::model::CodeGraph;

pub(crate) fn collect_sibling_tests(
    graph: &CodeGraph,
    changed: &[String],
    impacted: &[ImpactedItem],
    tests: &mut Vec<String>,
) {
    let mut targets: Vec<(&str, String)> = Vec::new();
    for f in changed.iter().chain(impacted.iter().map(|i| &i.file)) {
        if !is_test_file(f)
            && let Some(stem) = file_stem_name(f)
        {
            targets.push((parent_dir(f), stem));
        }
    }
    for candidate in graph.file_hashes.keys() {
        if is_test_file(candidate)
            && let Some(c_stem) = test_base_stem(candidate)
        {
            let cand_dir = parent_dir(candidate);
            if targets
                .iter()
                .any(|(src_dir, stem)| stem == &c_stem && is_sibling_test_dir(src_dir, cand_dir))
            {
                tests.push(candidate.clone());
            }
        }
    }
}

fn is_sibling_test_dir(src_dir: &str, cand_dir: &str) -> bool {
    if cand_dir == src_dir {
        return true;
    }
    let sub_tail = if src_dir.is_empty() {
        Some(cand_dir)
    } else {
        cand_dir
            .strip_prefix(src_dir)
            .and_then(|rest| rest.strip_prefix('/'))
    };
    if matches!(sub_tail, Some("__tests__" | "tests")) {
        return true;
    }
    if src_dir == "src" && cand_dir == "tests" {
        return true;
    }
    if let Some(pkg_dir) = src_dir.strip_suffix("/src")
        && let Some(rest) = cand_dir.strip_prefix(pkg_dir)
        && rest == "/tests"
    {
        return true;
    }
    is_php_laravel_test_dir(src_dir, cand_dir)
}

fn is_php_laravel_test_dir(src_dir: &str, cand_dir: &str) -> bool {
    let src_lower = src_dir.to_ascii_lowercase();
    let cand_lower = cand_dir.to_ascii_lowercase();
    let (src_sub, src_in_app) = split_laravel_subrepo(&src_lower, "app");
    let (cand_sub, cand_in_tests) = split_laravel_subrepo(&cand_lower, "tests");
    if src_in_app && cand_in_tests && src_sub == cand_sub {
        return true;
    }
    let (src_sub2, src_in_src) = split_laravel_subrepo(&src_lower, "src");
    src_in_src && cand_in_tests && src_sub2 == cand_sub
}

fn split_laravel_subrepo<'a>(dir: &'a str, root_seg: &str) -> (&'a str, bool) {
    if dir == root_seg || dir.starts_with(&format!("{root_seg}/")) {
        return ("", true);
    }
    let needle = format!("/{root_seg}");
    if let Some(idx) = dir.find(&needle) {
        let after = &dir[idx + needle.len()..];
        if after.is_empty() || after.starts_with('/') {
            return (&dir[..idx], true);
        }
    }
    ("", false)
}

fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

pub(crate) fn is_test_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let fname = lower.rsplit('/').next().unwrap_or(&lower);
    lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.contains("/__tests__/")
        || fname.starts_with("test_")
        || fname.ends_with("_test.py")
        || fname.ends_with("_test.dart")
        || fname.ends_with("test.kt")
        || fname.ends_with("test.php")
        || fname.ends_with("_test.rs")
        || fname.ends_with("_tests.rs")
        || fname.ends_with("_test.gd")
        || (lower.ends_with(".gd")
            && (lower.starts_with("test/")
                || lower.starts_with("tests/")
                || lower.contains("/test/")
                || lower.contains("/tests/")))
        || (lower.ends_with(".rs") && (lower.contains("/tests/") || lower.starts_with("tests/")))
}

pub(crate) fn is_screen_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("screen")
        || lower.contains("page")
        || lower.ends_with(".component.html")
        || lower.ends_with(".blade.php")
        || lower.contains("/views/")
}

fn file_stem_name(path: &str) -> Option<String> {
    let fname = path.rsplit('/').next()?;
    Some(fname.split('.').next()?.to_ascii_lowercase())
}

fn test_base_stem(path: &str) -> Option<String> {
    let fname = path.rsplit('/').next()?.to_ascii_lowercase();
    for suffix in [".test.", ".spec."] {
        if let Some((stem, _)) = fname.split_once(suffix) {
            return Some(stem.to_owned());
        }
    }
    if let Some(rest) = fname.strip_prefix("test_") {
        return rest.split('.').next().map(str::to_owned);
    }
    for suffix in [
        "_test.py",
        "_test.dart",
        "test.kt",
        "test.php",
        "_test.rs",
        "_tests.rs",
        "_test.gd",
    ] {
        if let Some(stem) = fname.strip_suffix(suffix) {
            return Some(stem.to_owned());
        }
    }
    None
}
