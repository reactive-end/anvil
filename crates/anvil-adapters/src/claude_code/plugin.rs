//! The Claude Code mod (`anvil-mode`) embedded in the binary and the Anvil-owned folder it is served from.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Marketplace name declared by the embedded `marketplace.json`.
pub const MARKETPLACE: &str = "anvil";
/// Plugin name inside the marketplace.
pub const PLUGIN: &str = "anvil-mode";
/// `plugin@marketplace` identifier used with `claude plugin`.
pub const PLUGIN_ID: &str = "anvil-mode@anvil";

/// Lists the files Anvil wrote, so only those are ever replaced or deleted.
const MANIFEST: &str = ".anvil-files.json";

macro_rules! embed {
    ($rel:literal) => {
        ($rel, include_str!(concat!("../../../../integrations/claude-code/", $rel)))
    };
}

/// Runtime files of the mod (relative path, content). Tests and `kit.ts` are development-only.
pub const FILES: &[(&str, &str)] = &[
    embed!(".claude-plugin/marketplace.json"),
    embed!("anvil-mode/.claude-plugin/plugin.json"),
    embed!("anvil-mode/hooks/catalog.ts"),
    embed!("anvil-mode/hooks/hooks.json"),
    embed!("anvil-mode/hooks/i18n.ts"),
    embed!("anvil-mode/hooks/language.ts"),
    embed!("anvil-mode/hooks/locale.ts"),
    embed!("anvil-mode/hooks/models.ts"),
    embed!("anvil-mode/hooks/register.tsx"),
    embed!("anvil-mode/hooks/view.tsx"),
    embed!("anvil-mode/types/index.d.ts"),
];

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    files: Vec<String>,
}

/// Version declared by the embedded `plugin.json`.
pub fn embedded_version() -> Result<String> {
    let body = FILES
        .iter()
        .find(|(rel, _)| *rel == "anvil-mode/.claude-plugin/plugin.json")
        .map(|(_, body)| *body)
        .context("embedded plugin.json is missing")?;
    let manifest: Value = serde_json::from_str(body).context("embedded plugin.json is invalid")?;
    manifest
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .context("embedded plugin.json has no version")
}

/// Writes the embedded marketplace into `dir` and removes files Anvil wrote earlier but no longer ships.
///
/// Refuses when `dir` holds anything Anvil did not write.
pub fn write_marketplace(dir: &Path) -> Result<()> {
    let previous = read_manifest(dir)?;
    ensure_owned(dir, &previous)?;
    // Listed before any file lands, so an interrupted install never leaves unlisted files behind.
    let mut listed = previous.clone();
    listed.extend(FILES.iter().map(|(rel, _)| (*rel).to_owned()).filter(|rel| !previous.contains(rel)));
    save_manifest(dir, listed)?;
    for (rel, body) in FILES {
        let target = dir.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(&target, body).with_context(|| format!("failed to write {}", target.display()))?;
    }
    for stale in previous.iter().filter(|old| !FILES.iter().any(|(rel, _)| rel == old)) {
        remove_listed(dir, stale)?;
    }
    prune_empty_dirs(dir)?;
    save_manifest(dir, FILES.iter().map(|(rel, _)| (*rel).to_owned()).collect())
}

fn save_manifest(dir: &Path, files: Vec<String>) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let json = serde_json::to_string_pretty(&Manifest { files }).context("failed to encode file manifest")?;
    fs::write(dir.join(MANIFEST), json).context("failed to write file manifest")
}

/// Deletes the files listed in the manifest, then the manifest and any directory left empty.
///
/// Returns how many listed files were removed; foreign files are never touched.
pub fn remove_marketplace(dir: &Path) -> Result<usize> {
    let listed = read_manifest(dir)?;
    let mut removed = 0;
    for rel in &listed {
        removed += usize::from(remove_listed(dir, rel)?);
    }
    if !listed.is_empty() {
        remove_listed(dir, MANIFEST)?;
    }
    prune_empty_dirs(dir)?;
    Ok(removed)
}

fn remove_listed(dir: &Path, rel: &str) -> Result<bool> {
    match fs::remove_file(dir.join(rel)) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).with_context(|| format!("failed to remove {rel}")),
    }
}

fn read_manifest(dir: &Path) -> Result<Vec<String>> {
    let raw = match fs::read_to_string(dir.join(MANIFEST)) {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).context("failed to read file manifest"),
    };
    let manifest: Manifest = serde_json::from_str(&raw).context("file manifest is invalid")?;
    if let Some(bad) = manifest.files.iter().find(|rel| !is_safe_relative(rel)) {
        bail!("file manifest lists an unsafe path: {bad}");
    }
    Ok(manifest.files)
}

fn is_safe_relative(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.contains(['\\', ':'])
        && rel.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

fn ensure_owned(dir: &Path, listed: &[String]) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    if !dir.is_dir() {
        bail!("{} exists and is not a folder", dir.display());
    }
    let mut found = Vec::new();
    collect_files(dir, dir, &mut found)?;
    match found.iter().find(|rel| rel.as_str() != MANIFEST && !listed.contains(rel)) {
        Some(foreign) => bail!(
            "{} holds content Anvil did not write ({foreign}); move it away or remove the folder",
            dir.display()
        ),
        None => Ok(()),
    }
}

fn collect_files(base: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_files(base, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(base) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// Removes empty directories under `dir` (and `dir` itself when it ends up empty).
fn prune_empty_dirs(dir: &Path) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            prune_empty_dirs(&entry.path())?;
        }
    }
    if fs::read_dir(dir)?.next().is_none() {
        fs::remove_dir(dir).with_context(|| format!("failed to remove {}", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_development_only(name: &str) -> bool {
        name == "kit.ts" || name.contains(".test.")
    }

    fn collect_shipped(base: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if path.is_dir() {
                if name != "node_modules" {
                    collect_shipped(base, &path, out)?;
                }
            } else if !is_development_only(&name) {
                out.push(path.strip_prefix(base)?.to_string_lossy().replace('\\', "/"));
            }
        }
        Ok(())
    }

    #[test]
    fn every_shipped_file_of_the_mod_is_embedded() -> Result<()> {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../integrations/claude-code");
        let mut shipped = Vec::new();
        collect_shipped(&base, &base, &mut shipped)?;
        let missing: Vec<&String> =
            shipped.iter().filter(|rel| !FILES.iter().any(|(known, _)| known == rel)).collect();
        assert!(missing.is_empty(), "add these files to FILES in plugin.rs: {missing:?}");
        Ok(())
    }

    #[test]
    fn embedded_version_reads_the_plugin_manifest() -> Result<()> {
        assert!(!embedded_version()?.is_empty());
        Ok(())
    }

    #[test]
    fn write_replaces_stale_files_and_refuses_foreign_content() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let dir = tmp.path().join("plugin");
        write_marketplace(&dir)?;
        let mut files: Vec<String> = FILES.iter().map(|(rel, _)| (*rel).to_owned()).collect();
        files.push("old/gone.ts".into());
        fs::write(dir.join(MANIFEST), serde_json::to_string(&Manifest { files })?)?;
        fs::create_dir_all(dir.join("old"))?;
        fs::write(dir.join("old/gone.ts"), "x")?;
        write_marketplace(&dir)?;
        assert!(!dir.join("old").exists());
        assert!(dir.join("anvil-mode/hooks/register.tsx").is_file());

        fs::write(dir.join("mine.txt"), "keep")?;
        assert!(write_marketplace(&dir).is_err());
        assert_eq!(remove_marketplace(&dir)?, FILES.len());
        assert_eq!(fs::read_to_string(dir.join("mine.txt"))?, "keep");
        Ok(())
    }

    #[test]
    fn an_interrupted_install_does_not_block_the_next_one() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let dir = tmp.path().join("plugin");
        let early: Vec<String> = FILES.iter().map(|(rel, _)| (*rel).to_owned()).collect();
        save_manifest(&dir, early)?;
        let (rel, body) = FILES[2];
        fs::create_dir_all(dir.join(rel).parent().context("file has a parent")?)?;
        fs::write(dir.join(rel), body)?;

        write_marketplace(&dir)?;
        assert_eq!(remove_marketplace(&dir)?, FILES.len());
        assert!(!dir.exists());
        Ok(())
    }

    #[test]
    fn manifest_cannot_point_outside_the_folder() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let dir = tmp.path().join("plugin");
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(MANIFEST), r#"{"files":["../victim.txt"]}"#)?;
        assert!(remove_marketplace(&dir).is_err());
        Ok(())
    }
}
