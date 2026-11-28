//! Settings that belong to the person, not to a repository: `<home>/.anvil/config.json`, where
//! `<home>` is the folder that holds `.cargo` (`%USERPROFILE%` on Windows, `$HOME` elsewhere).
//! `ANVIL_HOME` replaces the whole `.anvil` directory, which keeps tests off the real home.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::config::prompt_assist::PromptAssistConfig;
use crate::fs_util::atomic_write;

/// Environment variable that replaces the `.anvil` directory.
pub const ANVIL_HOME_ENV: &str = "ANVIL_HOME";

/// Global settings shared by every project of this user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserConfig {
    /// What the prompt hook adds to each prompt (`anvil prompt assist`); off until turned on.
    #[serde(default, skip_serializing_if = "PromptAssistConfig::is_default")]
    pub prompt_assist: PromptAssistConfig,
}

/// The user's home directory: `USERPROFILE`, else `HOME`.
pub fn home_dir() -> Option<PathBuf> {
    home_from(std::env::var_os("USERPROFILE"), std::env::var_os("HOME"))
}

/// The `.anvil` directory: `ANVIL_HOME` when set, else `<home>/.anvil`.
pub fn anvil_dir() -> Option<PathBuf> {
    dir_from(std::env::var_os(ANVIL_HOME_ENV), home_dir())
}

/// Path of the global config file, or an error when no home directory can be found.
pub fn config_path() -> Result<PathBuf> {
    anvil_dir().map(|dir| dir.join("config.json")).ok_or_else(|| {
        anyhow!("cannot find the home directory (set USERPROFILE, HOME, or {ANVIL_HOME_ENV})")
    })
}

fn home_from(userprofile: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    [userprofile, home].into_iter().flatten().find(|v| !v.is_empty()).map(PathBuf::from)
}

fn dir_from(anvil_home: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    match anvil_home.filter(|v| !v.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => home.map(|h| h.join(".anvil")),
    }
}

impl UserConfig {
    /// Loads the global config; a missing file means defaults, an unreadable or invalid one fails.
    pub fn load() -> Result<Self> {
        Self::load_from(&config_path()?)
    }

    /// Writes the global config, creating the `.anvil` directory only now.
    pub fn save(&self) -> Result<()> {
        self.save_to(&config_path()?)
    }

    /// Like [`UserConfig::load`] for an explicit file.
    pub fn load_from(path: &Path) -> Result<Self> {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => {
                return Err(err).with_context(|| format!("failed to read {}", path.display()));
            }
        };
        let clean = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
        serde_json::from_str(clean).with_context(|| format!("invalid JSON in {}", path.display()))
    }

    /// Like [`UserConfig::save`] for an explicit file; only non-default values are written.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self).context("failed to serialize config")?;
        atomic_write(path, format!("{json}\n").as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::prompt_assist::PromptAssistMode;

    fn os(text: &str) -> Option<OsString> {
        Some(OsString::from(text))
    }

    #[test]
    fn anvil_home_replaces_the_whole_directory() {
        let dir = dir_from(os("/tmp/custom"), Some(PathBuf::from("/home/u")));
        assert_eq!(dir, Some(PathBuf::from("/tmp/custom")));
    }

    #[test]
    fn without_anvil_home_the_directory_sits_next_to_cargo_in_the_home() {
        let dir = dir_from(None, Some(PathBuf::from("/home/u")));
        assert_eq!(dir, Some(PathBuf::from("/home/u").join(".anvil")));
        assert_eq!(dir_from(os(""), Some(PathBuf::from("/home/u"))), dir);
        assert_eq!(dir_from(None, None), None);
    }

    #[test]
    fn the_home_is_userprofile_first_then_home() {
        assert_eq!(home_from(os("C:\\Users\\a"), os("/h")), Some(PathBuf::from("C:\\Users\\a")));
        assert_eq!(home_from(None, os("/h")), Some(PathBuf::from("/h")));
        assert_eq!(home_from(os(""), os("/h")), Some(PathBuf::from("/h")));
        assert_eq!(home_from(None, None), None);
    }

    #[test]
    fn a_missing_file_means_prompt_assist_is_off() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = UserConfig::load_from(&tmp.path().join(".anvil/config.json")).unwrap();
        assert_eq!(cfg.prompt_assist.mode, PromptAssistMode::Off);
        assert!(!tmp.path().join(".anvil").exists(), "loading never creates the directory");
    }

    #[test]
    fn saving_creates_the_directory_and_writes_only_non_default_values() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join(".anvil/config.json");
        let mut cfg = UserConfig::default();
        cfg.save_to(&path).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&path).unwrap()).unwrap(),
            serde_json::json!({})
        );
        cfg.prompt_assist.mode = PromptAssistMode::Coach;
        cfg.save_to(&path).unwrap();
        assert_eq!(UserConfig::load_from(&path).unwrap(), cfg);
        cfg.prompt_assist = PromptAssistConfig::default();
        cfg.save_to(&path).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("prompt_assist"), "{saved}");
    }

    #[test]
    fn a_repo_config_that_still_has_a_prompt_assist_section_loads() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = crate::config::AnvilConfig::path(tmp.path());
        fs::create_dir_all(repo.parent().unwrap()).unwrap();
        fs::write(&repo, r#"{ "prompt_assist": { "mode": "coach" } }"#).unwrap();
        let cfg = crate::config::AnvilConfig::load(tmp.path()).unwrap();
        assert!(!serde_json::to_string(&cfg).unwrap().contains("prompt_assist"));
    }

    #[test]
    fn invalid_json_is_an_error_not_a_silent_reset() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        fs::write(&path, "{ not json").unwrap();
        let err = UserConfig::load_from(&path).unwrap_err();
        assert!(format!("{err:#}").contains("invalid JSON"), "{err:#}");
    }
}
