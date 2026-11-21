//! Canonical filesystem, locking, path, JSONC, and binary resolution utilities.

pub mod jsonc;
pub mod path;

pub use jsonc::strip_jsonc;
pub use path::{bin_in_path, relativize_path, resolve_local_or_path_bin, sanitize_author};

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use ulid::Ulid;

const STALE_LOCK_SECS: u64 = 30;

/// Atomically writes `bytes` to `path` via a unique temp file and rename.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create directory {}", parent.display()))?;

    let tmp_path = parent.join(format!(".tmp.{}.{}", std::process::id(), Ulid::new()));
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
            .with_context(|| format!("failed to create temp file {}", tmp_path.display()))?;
        if let Err(err) = file.write_all(bytes) {
            drop(file);
            let _ = fs::remove_file(&tmp_path);
            return Err(err)
                .with_context(|| format!("failed to write temp file {}", tmp_path.display()));
        }
    }

    rename_with_replace_fallback(&tmp_path, path)
}

fn rename_with_replace_fallback(tmp_path: &Path, target: &Path) -> Result<()> {
    let mut last_err = None;
    for attempt in 0..5u64 {
        match fs::rename(tmp_path, target) {
            Ok(()) => return Ok(()),
            Err(err) => {
                if target.exists() {
                    let _ = fs::remove_file(target);
                    if fs::rename(tmp_path, target).is_ok() {
                        return Ok(());
                    }
                }
                let retry = cfg!(windows) && err.kind() == std::io::ErrorKind::PermissionDenied;
                last_err = Some(err);
                if !retry || attempt == 4 {
                    break;
                }
                thread::sleep(Duration::from_millis(2 * (1 << attempt)));
            }
        }
    }
    let _ = fs::remove_file(tmp_path);
    Err(last_err.unwrap_or_else(|| std::io::Error::other("rename failed"))).with_context(|| {
        format!(
            "failed to atomically rename {} -> {}",
            tmp_path.display(),
            target.display()
        )
    })
}

struct LockGuard(PathBuf);

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Acquires a cross-process advisory lock at `lock_path`, runs `f`, and releases via RAII.
pub fn with_file_lock<T, F: FnOnce() -> Result<T>>(
    lock_path: &Path,
    timeout_ms: u64,
    f: F,
) -> Result<T> {
    if let Some(parent) = lock_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create lock directory {}", parent.display()))?;
    }

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut backoff_ms = 2u64;
    let mut attempt = 0u64;

    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock_path)
        {
            Ok(mut file) => {
                let _ = writeln!(file, "{} {}", std::process::id(), Ulid::new());
                drop(file);
                let _guard = LockGuard(lock_path.to_path_buf());
                return f();
            }
            Err(err) if is_lock_contention(&err) => {
                if err.kind() == std::io::ErrorKind::AlreadyExists
                    && try_recover_stale_lock(lock_path)
                {
                    continue;
                }
                let now = Instant::now();
                if now >= deadline {
                    bail!(
                        "timed out acquiring file lock {} after {timeout_ms}ms",
                        lock_path.display()
                    );
                }
                let jitter = (u64::from(std::process::id()) ^ attempt) % 5;
                let sleep_for = Duration::from_millis(backoff_ms + jitter)
                    .min(deadline.saturating_duration_since(now));
                thread::sleep(sleep_for);
                backoff_ms = (backoff_ms * 2).min(25);
                attempt = attempt.saturating_add(1);
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to create lock {}", lock_path.display()));
            }
        }
    }
}

fn is_lock_contention(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::AlreadyExists
        || (cfg!(windows) && err.kind() == std::io::ErrorKind::PermissionDenied)
}

fn try_recover_stale_lock(lock_path: &Path) -> bool {
    let Ok(meta) = fs::metadata(lock_path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return false;
    };
    if age > Duration::from_secs(STALE_LOCK_SECS) {
        return fs::remove_file(lock_path).is_ok();
    }
    false
}
