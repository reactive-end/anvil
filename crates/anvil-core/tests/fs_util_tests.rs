use std::fs::{self, OpenOptions};
use std::path::Path;
use std::sync::Arc;
use std::thread;

use anyhow::Result;
use tempfile::TempDir;

use anvil_core::fs_util::{
    atomic_write, relativize_path, resolve_local_or_path_bin, sanitize_author, strip_jsonc,
    with_file_lock,
};

#[test]
fn atomic_write_creates_replaces_and_cleans_temp_files() -> Result<()> {
    let tmp = TempDir::new()?;
    let target = tmp.path().join("nested").join("data.json");

    atomic_write(&target, b"{\"v\":1}\n")?;
    assert_eq!(fs::read_to_string(&target)?, "{\"v\":1}\n");

    atomic_write(&target, b"{\"v\":2}\n")?;
    assert_eq!(fs::read_to_string(&target)?, "{\"v\":2}\n");

    let entries: Vec<_> = fs::read_dir(target.parent().expect("parent"))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec!["data.json"]);
    Ok(())
}

#[test]
fn with_file_lock_serializes_threads_and_releases_on_error() -> Result<()> {
    let tmp = TempDir::new()?;
    let lock = Arc::new(tmp.path().join("state.lock"));
    let counter = Arc::new(tmp.path().join("count.txt"));
    fs::write(counter.as_path(), "0")?;

    let mut handles = Vec::new();
    for _ in 0..4 {
        let lock_c = Arc::clone(&lock);
        let counter_c = Arc::clone(&counter);
        handles.push(thread::spawn(move || -> Result<()> {
            for _ in 0..5 {
                with_file_lock(&lock_c, 2_000, || {
                    let cur: u32 = fs::read_to_string(counter_c.as_path())?.trim().parse()?;
                    fs::write(counter_c.as_path(), (cur + 1).to_string())?;
                    Ok(())
                })?;
            }
            Ok(())
        }));
    }
    for h in handles {
        h.join().expect("thread join")?;
    }
    assert_eq!(fs::read_to_string(counter.as_path())?, "20");

    // OS advisory lock is released immediately after closure returns or errors.
    let probe = OpenOptions::new().read(true).write(true).open(lock.as_path())?;
    assert!(probe.try_lock().is_ok());
    drop(probe);

    let err_res: Result<()> = with_file_lock(&lock, 500, || anyhow::bail!("boom"));
    assert!(err_res.is_err());

    let probe_after_err = OpenOptions::new().read(true).write(true).open(lock.as_path())?;
    assert!(probe_after_err.try_lock().is_ok());
    drop(probe_after_err);

    // Held OS advisory lock blocks concurrent acquisition until timeout.
    let held = OpenOptions::new().create(true).read(true).write(true).truncate(false).open(lock.as_path())?;
    held.try_lock().expect("acquire outer lock");
    let timed_out: Result<()> = with_file_lock(&lock, 40, || Ok(()));
    assert!(timed_out.is_err());
    drop(held);

    with_file_lock(&lock, 500, || Ok(()))?;
    Ok(())
}

#[test]
fn strip_jsonc_handles_comments_trailing_commas_and_strings() {
    let input = r#"{
        // single line comment
        "url": "https://example.com/*not-a-comment*/,//keep",
        /* multi
           line comment */
        "arr": [1, 2, ],
    }"#;
    let stripped = strip_jsonc(input);
    let parsed: serde_json::Value = serde_json::from_str(&stripped).expect("valid JSON");
    assert_eq!(parsed["url"], "https://example.com/*not-a-comment*/,//keep");
    assert_eq!(parsed["arr"], serde_json::json!([1, 2]));
}

#[test]
fn path_author_and_local_bin_helpers_work_portably() -> Result<()> {
    assert_eq!(sanitize_author("  Ada Lovelace! "), "Ada-Lovelace");
    assert_eq!(sanitize_author("$$$"), "unknown");

    assert_eq!(
        relativize_path(Path::new(r"C:\Repo\App"), Path::new(r"c:\repo\app\src\main.rs")),
        "src/main.rs"
    );
    assert_eq!(
        relativize_path(Path::new("/repo"), Path::new("/repo_other/a.rs")),
        "/repo_other/a.rs"
    );

    let tmp = TempDir::new()?;
    let bin_dir = tmp.path().join("node_modules").join(".bin");
    fs::create_dir_all(&bin_dir)?;
    let tool_name = if cfg!(windows) { "mytool.cmd" } else { "mytool" };
    fs::write(bin_dir.join(tool_name), "echo ok")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(bin_dir.join(tool_name), fs::Permissions::from_mode(0o755))?;
    }

    let resolved = resolve_local_or_path_bin(tmp.path(), "mytool").expect("local bin");
    assert_eq!(resolved.parent(), Some(bin_dir.as_path()));
    assert!(
        resolved
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case(tool_name))
    );
    Ok(())
}
