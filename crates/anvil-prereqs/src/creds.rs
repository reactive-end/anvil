//! Finds `scheme://user:password@host` connection strings. Only the scheme and host are ever returned.

use std::sync::OnceLock;

use regex::Regex;

/// The non-secret part of a connection string that embeds a password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineCredential {
    pub scheme: String,
    pub host: String,
}

impl InlineCredential {
    /// `postgres://db.internal`: safe to print.
    pub fn label(&self) -> String {
        format!("{}://{}", self.scheme, self.host)
    }
}

fn pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"([A-Za-z][A-Za-z0-9+.\-]*)://([^\s/:@"'<>]+):([^\s/@"'<>]+)@([^\s/:?#"'<>]+)"#,
        )
        .expect("static pattern")
    })
}

/// Variable references (`$DB_PASS`, `${DB_PASS}`, `%DB_PASS%`, `{{pass}}`, `<password>`) are not secrets.
fn is_reference(password: &str) -> bool {
    password.starts_with(['$', '%', '{', '<']) || password.contains("${") || password.contains("{{")
}

/// Connection strings with a literal password in `text`.
pub fn find_credentials(text: &str) -> Vec<InlineCredential> {
    pattern()
        .captures_iter(text)
        .filter(|c| !is_reference(&c[3]))
        .map(|c| InlineCredential { scheme: c[1].to_ascii_lowercase(), host: c[4].to_owned() })
        .collect()
}
