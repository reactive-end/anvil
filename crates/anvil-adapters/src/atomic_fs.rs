//! Fast atomic file writes and directory creation helpers.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Ensures `dir` and all missing parent directories exist.
pub fn ensure_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)
        .with_context(|| format!("failed to create directory {}", dir.display()))
}

/// Atomically writes `bytes` to `path` via a same-directory temporary file and rename.
pub fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("target path must have a parent directory")?;
    ensure_dir(parent)?;

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("anvil");
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!(".{file_name}.tmp.{}.{seq}", std::process::id());
    let tmp_path = parent.join(tmp_name);

    fs::write(&tmp_path, bytes)
        .with_context(|| format!("failed to write temp file {}", tmp_path.display()))?;

    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err).with_context(|| {
            format!(
                "failed to atomically rename {} -> {}",
                tmp_path.display(),
                path.display()
            )
        });
    }
    Ok(())
}

/// Writes `content` atomically only when `path` is missing or its contents differ.
/// Returns `true` when the file on disk was created or updated.
pub fn write_if_changed(path: &Path, content: &str) -> Result<bool> {
    if let Ok(existing) = fs::read_to_string(path)
        && existing == content
    {
        return Ok(false);
    }
    atomic_write_bytes(path, content.as_bytes())?;
    Ok(true)
}
