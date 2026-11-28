//! Shared helpers: temp git repositories and bundles.

use std::fs;
use std::path::Path;
use std::process::Command;

use anvil_docs::bundle::Bundle;
use tempfile::TempDir;

pub fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().expect("run git");
    assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
}

/// A repository with one commit holding `src/orders/service.ts` and `src/billing/invoice.ts`.
pub fn repo() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "t@example.com"]);
    git(root, &["config", "user.name", "Tester"]);
    write(root, "src/orders/service.ts", "export function createOrder() {}\n");
    write(root, "src/billing/invoice.ts", "export function bill() {}\n");
    commit(root, "initial");
    dir
}

pub fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, text).expect("write");
}

pub fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", message]);
}

pub fn bundle(root: &Path) -> Bundle {
    Bundle {
        project: "shop".to_owned(),
        dir: root.join(".harness/docs/shop"),
        code_root: String::new(),
        nested: Vec::new(),
    }
}

pub fn index_text() -> String {
    "---\nokf_version: \"0.3\"\n---\n\n# shop\n\n## Modules\n".to_owned()
}
