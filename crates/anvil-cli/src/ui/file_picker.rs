//! Native OS file dialog and Clack-style TUI directory/file browser for picking JSON files.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::ui::prompt::{self, PromptOutcome, SelectOption};

/// A single entry displayed inside the interactive TUI file browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserEntry {
    pub path: PathBuf,
    pub label: String,
    pub hint: String,
    pub is_dir: bool,
}

/// Opens a file chooser starting at `start_dir` to select a `.json` file.
/// Uses native Windows File Explorer / macOS Finder / Linux Zenity when available,
/// or falls back to the built-in interactive TUI file browser when `force_tui` is set.
pub fn pick_json_file(start_dir: &Path, force_tui: bool) -> Result<PromptOutcome<PathBuf>> {
    if force_tui {
        return browse_file_tui(start_dir, "json");
    }
    #[cfg(windows)]
    {
        pick_windows_dialog(start_dir)
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(res) = try_macos_osascript() {
            Ok(res)
        } else {
            browse_file_tui(start_dir, "json")
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(res) = try_linux_zenity(start_dir) {
            Ok(res)
        } else {
            browse_file_tui(start_dir, "json")
        }
    }
}

#[cfg(windows)]
fn pick_windows_dialog(start_dir: &Path) -> Result<PromptOutcome<PathBuf>> {
    let dir = start_dir
        .canonicalize()
        .unwrap_or_else(|_| start_dir.to_path_buf());
    let picked = rfd::FileDialog::new()
        .set_title("Anvil — Select JSON task/feature list")
        .add_filter("JSON files (*.json)", &["json"])
        .add_filter("All files (*.*)", &["*"])
        .set_directory(&dir)
        .pick_file();
    Ok(match picked {
        Some(path) => {
            if prompt::is_interactive_tui() {
                let _ = prompt::emit_collapsed("Selected JSON file", &path.display().to_string());
            }
            PromptOutcome::Confirmed(path)
        }
        None => PromptOutcome::Cancelled,
    })
}

#[cfg(target_os = "macos")]
fn try_macos_osascript() -> Option<PromptOutcome<PathBuf>> {
    let out = std::process::Command::new("osascript")
        .args([
            "-e",
            r#"POSIX path of (choose file of type {"public.json"} with prompt "Anvil — Select JSON task/feature list")"#,
        ])
        .output()
        .ok()?;
    if out.status.success() {
        let p = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !p.is_empty() {
            return Some(PromptOutcome::Confirmed(PathBuf::from(p)));
        }
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if err.contains("User canceled") || err.contains("-128") {
        return Some(PromptOutcome::Cancelled);
    }
    None
}

#[cfg(all(unix, not(target_os = "macos")))]
fn try_linux_zenity(start_dir: &Path) -> Option<PromptOutcome<PathBuf>> {
    let has_gui = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    if !has_gui {
        return None;
    }
    let filename_arg = format!("{}/", start_dir.display());
    let out = std::process::Command::new("zenity")
        .args([
            "--file-selection",
            "--title=Anvil — Select JSON task/feature list",
            "--filename",
            &filename_arg,
            "--file-filter=JSON files (*.json) | *.json",
            "--file-filter=All files | *",
        ])
        .output()
        .ok()?;
    if out.status.success() {
        let p = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !p.is_empty() {
            return Some(PromptOutcome::Confirmed(PathBuf::from(p)));
        }
    }
    if out.status.code() == Some(1) {
        return Some(PromptOutcome::Cancelled);
    }
    None
}

/// Interactive terminal folder/file navigator built on Anvil's Clack-style arrow-key selector.
pub fn browse_file_tui(start_dir: &Path, ext_filter: &str) -> Result<PromptOutcome<PathBuf>> {
    if !prompt::is_interactive_tui() {
        bail!("interactive file browser requires a TTY; pass `<file>` path directly instead");
    }
    let mut cur_dir = start_dir
        .canonicalize()
        .or_else(|_| std::env::current_dir())
        .context("failed to resolve starting folder for file browser")?;

    loop {
        let entries = list_browser_entries(&cur_dir, ext_filter)?;
        let options: Vec<SelectOption> = entries
            .iter()
            .map(|e| SelectOption::new(&e.label, &e.hint))
            .collect();
        let sub = format!("Folder: {}", cur_dir.display());
        let outcome = prompt::select_transient(
            "Select JSON feature/task list",
            Some(&sub),
            &options,
            0,
        )?;
        match outcome {
            PromptOutcome::Cancelled => return Ok(PromptOutcome::Cancelled),
            PromptOutcome::Confirmed(idx) => {
                let chosen = &entries[idx];
                if chosen.is_dir {
                    cur_dir = chosen.path.clone();
                } else {
                    prompt::emit_collapsed("Selected JSON file", &chosen.path.display().to_string())?;
                    return Ok(PromptOutcome::Confirmed(chosen.path.clone()));
                }
            }
        }
    }
}

/// Lists navigable parent (`..`), subdirectories, and files matching `ext_filter` (e.g. `"json"`).
pub fn list_browser_entries(dir: &Path, ext_filter: &str) -> Result<Vec<BrowserEntry>> {
    let mut entries = Vec::new();
    if let Some(parent) = dir.parent() {
        entries.push(BrowserEntry {
            path: parent.to_path_buf(),
            label: "..".to_owned(),
            hint: "parent directory".to_owned(),
            is_dir: true,
        });
    }

    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let rd = fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?;
    for item in rd.flatten() {
        let path = item.path();
        let name = item.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            dirs.push(BrowserEntry {
                path,
                label: format!("[dir] {name}/"),
                hint: "open folder".to_owned(),
                is_dir: true,
            });
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case(ext_filter))
        {
            let size_hint = fs::metadata(&path)
                .map(|m| format_size(m.len()))
                .unwrap_or_default();
            files.push(BrowserEntry {
                path,
                label: name,
                hint: size_hint,
                is_dir: false,
            });
        }
    }

    dirs.sort_by_key(|a| a.label.to_ascii_lowercase());
    files.sort_by_key(|a| a.label.to_ascii_lowercase());
    entries.extend(dirs);
    entries.extend(files);
    Ok(entries)
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn browser_lists_parent_directories_then_matching_json_files() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("alpha_dir");
        fs::create_dir_all(&sub).unwrap();
        fs::write(tmp.path().join("feature_list.json"), "{}").unwrap();
        fs::write(tmp.path().join("notes.txt"), "ignore").unwrap();

        let entries = list_browser_entries(tmp.path(), "json").unwrap();
        assert_eq!(entries[0].label, "..");
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].label, "[dir] alpha_dir/");
        assert!(entries[1].is_dir);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[2].label, "feature_list.json");
        assert!(!entries[2].is_dir);
    }
}
