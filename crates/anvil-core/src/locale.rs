//! Language of the text Anvil shows the user: Spanish or English.

use std::path::PathBuf;

use crate::config::AnvilConfig;

/// A language Anvil's user-facing messages are written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    Es,
}

impl Lang {
    /// Reads a language tag or name (`es-CL`, `es_ES.UTF-8`, `spanish`, `Español`); `None` when unrecognised.
    pub fn parse(raw: &str) -> Option<Self> {
        let lower = raw.trim().to_lowercase();
        if lower.is_empty() || lower == "c" || lower == "posix" {
            return None;
        }
        let es = (lower.starts_with("es")
            && lower[2..].chars().next().is_none_or(|c| !c.is_ascii_alphabetic()))
            || lower.starts_with("spanish")
            || lower.starts_with("español")
            || lower.starts_with("espanol")
            || lower.starts_with("castellano");
        if es {
            return Some(Self::Es);
        }
        let en = (lower.starts_with("en")
            && lower[2..].chars().next().is_none_or(|c| !c.is_ascii_alphabetic()))
            || lower.starts_with("english");
        en.then_some(Self::En)
    }

    /// Picks the language: `config.language`, then `ANVIL_LANG`, then Claude Code's `language`
    /// setting, then the locale environment variables, then the operating system's UI language;
    /// English otherwise.
    pub fn detect(config: &AnvilConfig) -> Self {
        let from_env = |key: &str| std::env::var(key).ok().and_then(|value| Self::parse(&value));
        config
            .language
            .as_deref()
            .and_then(Self::parse)
            .or_else(|| from_env("ANVIL_LANG"))
            .or_else(claude_setting)
            .or_else(|| ["LC_ALL", "LC_MESSAGES", "LANG"].into_iter().find_map(from_env))
            .or_else(|| sys_locale::get_locale().as_deref().and_then(Self::parse))
            .unwrap_or_default()
    }

    /// Returns the Spanish text for `Es`, the English text otherwise.
    pub fn pick<'a>(self, en: &'a str, es: &'a str) -> &'a str {
        match self {
            Self::En => en,
            Self::Es => es,
        }
    }
}

fn claude_setting() -> Option<Lang> {
    let raw = std::fs::read_to_string(claude_settings_path()?).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Lang::parse(value.get("language")?.as_str()?)
}

fn claude_settings_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("settings.json"));
    }
    Some(crate::user_config::home_dir()?.join(".claude").join("settings.json"))
}

#[cfg(test)]
mod tests {
    use super::Lang;

    #[test]
    fn parses_tags_and_names() {
        assert_eq!(Lang::parse("es-CL"), Some(Lang::Es));
        assert_eq!(Lang::parse("es_ES.UTF-8"), Some(Lang::Es));
        assert_eq!(Lang::parse("Spanish"), Some(Lang::Es));
        assert_eq!(Lang::parse("español"), Some(Lang::Es));
        assert_eq!(Lang::parse("en-US"), Some(Lang::En));
        assert_eq!(Lang::parse("english"), Some(Lang::En));
    }

    #[test]
    fn rejects_unknown_and_lookalikes() {
        assert_eq!(Lang::parse(""), None);
        assert_eq!(Lang::parse("C"), None);
        assert_eq!(Lang::parse("fr-FR"), None);
        assert_eq!(Lang::parse("estonian"), None);
        assert_eq!(Lang::parse("enigma"), None);
    }

    #[test]
    fn config_language_wins() {
        let config = anvil_config_with("es");
        assert_eq!(Lang::detect(&config), Lang::Es);
        assert_eq!(Lang::Es.pick("a", "b"), "b");
    }

    fn anvil_config_with(language: &str) -> crate::config::AnvilConfig {
        crate::config::AnvilConfig {
            language: Some(language.to_owned()),
            ..crate::config::AnvilConfig::default()
        }
    }
}
