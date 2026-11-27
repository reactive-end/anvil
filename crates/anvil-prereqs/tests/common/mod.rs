//! Shared fixtures: write files into a temporary workspace.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

pub fn workspace() -> TempDir {
    TempDir::new().expect("tempdir")
}

pub fn write(root: &Path, rel: &str, content: impl AsRef<[u8]>) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, content).expect("write");
}
