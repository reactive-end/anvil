//! Bounded hook error log persisted at `.harness/.state/hook-errors.log`.

use std::fmt::Display;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use anvil_core::fs_util::atomic_write;
use chrono::Utc;

const MAX_LOG_BYTES: usize = 64 * 1024;
const KEEP_LOG_BYTES: usize = 32 * 1024;

/// Appends `[<rfc3339>] [<stage>] <err>\n` to `.harness/.state/hook-errors.log` when `.harness` exists.
/// Atomically trims the log to the newest 32 KiB if it exceeds 64 KiB.
pub fn log_hook_error(root: &Path, stage: &str, err: &dyn Display) {
    if !root.join(".harness").is_dir() {
        return;
    }
    let log_path = root.join(".harness").join(".state").join("hook-errors.log");
    if let Some(parent) = log_path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return;
    }

    let line = format!("[{}] [{stage}] {err}\n", Utc::now().to_rfc3339());
    let existing_len = fs::metadata(&log_path)
        .map(|m| usize::try_from(m.len()).unwrap_or(usize::MAX))
        .unwrap_or(0);

    if existing_len.saturating_add(line.len()) > MAX_LOG_BYTES {
        let mut buf = fs::read(&log_path).unwrap_or_default();
        buf.extend_from_slice(line.as_bytes());
        let mut start = buf.len().saturating_sub(KEEP_LOG_BYTES);
        if start > 0 {
            let last_line_end = buf.len() - 1;
            if let Some(nl) = buf[start..last_line_end].iter().position(|&b| b == b'\n') {
                start += nl + 1;
            } else {
                while start < buf.len() && (buf[start] & 0b1100_0000) == 0b1000_0000 {
                    start += 1;
                }
            }
        }
        let _ = atomic_write(&log_path, &buf[start..]);
        return;
    }

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = file.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{KEEP_LOG_BYTES, MAX_LOG_BYTES, log_hook_error};

    #[test]
    fn skips_when_harness_dir_is_absent() {
        let dir = TempDir::new().unwrap();
        log_hook_error(dir.path(), "test-stage", &"boom");
        assert!(!dir.path().join(".harness").exists());
    }

    #[test]
    fn writes_and_bounds_hook_error_log() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join(".harness/.state")).unwrap();
        let log_path = dir.path().join(".harness/.state/hook-errors.log");

        log_hook_error(dir.path(), "session-start", &"initial error");
        let content = fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("[session-start] initial error"));

        let filler = "x".repeat(MAX_LOG_BYTES + 128);
        fs::write(&log_path, filler).unwrap();

        log_hook_error(dir.path(), "post-tool-use", &"latest failure");
        let meta = fs::metadata(&log_path).unwrap();
        assert!(
            usize::try_from(meta.len()).unwrap() <= KEEP_LOG_BYTES,
            "expected log size <= {KEEP_LOG_BYTES}, got {}",
            meta.len()
        );
        let trimmed = fs::read_to_string(&log_path).unwrap();
        assert!(trimmed.contains("[post-tool-use] latest failure"));

        // Multibyte UTF-8 log trimming never splits a codepoint or starts mid-line.
        let mb_line = "[2026-11-25T00:00:00Z] [utf8] fallo crítico en diseño\n";
        let repeated = mb_line.repeat(2_000);
        fs::write(&log_path, repeated).unwrap();
        log_hook_error(dir.path(), "post-tool-use", &"after utf8 trim");
        let utf8_ok = fs::read_to_string(&log_path).expect("trimmed log must stay valid UTF-8");
        assert!(utf8_ok.starts_with('['));
        assert!(utf8_ok.contains("after utf8 trim"));
    }
}
