//! Secret and `<private>` block redaction executed before any disk or DB write.

use std::borrow::Cow;
use std::sync::OnceLock;

use regex::Regex;

use crate::model::{MemoryChunkRecord, ObservationInput};

fn compile_regex(pattern: &str) -> Option<Regex> {
    Regex::new(pattern).ok()
}

fn private_block_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"(?is)<private>.*?</private>"))
        .as_ref()
}

fn pem_private_key_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(
            r"(?is)-----BEGIN [A-Z0-9 _-]*PRIVATE KEY-----.*?-----END [A-Z0-9 _-]*PRIVATE KEY-----",
        )
    })
    .as_ref()
}

fn conn_password_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"(?i)([a-z][a-z0-9+.-]*://[^:/\s@]*:)([^@\s]+)(@)"))
        .as_ref()
}

fn bearer_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"(?i)\bBearer\s+[A-Za-z0-9\-._~+/]+=*"))
        .as_ref()
}

fn jwt_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b")
    })
    .as_ref()
}

fn github_token_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b")
    })
    .as_ref()
}

fn aws_key_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"))
        .as_ref()
}

fn gcp_key_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"\bAIza[0-9A-Za-z\-_]{35}\b"))
        .as_ref()
}

fn stripe_key_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}\b"))
        .as_ref()
}

fn slack_token_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b"))
        .as_ref()
}

fn api_key_prefix_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"\bsk-[A-Za-z0-9_\-]{20,}\b"))
        .as_ref()
}

fn secret_kv_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(
            r#"(?i)\b(aws_secret_access_key|app[_-]?key|api[_-]?key|secret[_-]?key|access[_-]?token|auth[_-]?token|client[_-]?secret|password|passwd|pwd)\b(\s*[:=]\s*["']?)([^\s"'`,;]{4,})(["']?)"#,
        )
    })
    .as_ref()
}

fn apply_replace<'a>(text: Cow<'a, str>, re: Option<&Regex>, rep: &str) -> Cow<'a, str> {
    let Some(regex) = re else {
        return text;
    };
    match regex.replace_all(&text, rep) {
        Cow::Borrowed(_) => text,
        Cow::Owned(updated) => Cow::Owned(updated),
    }
}

/// Strips `<private>...</private>` blocks and redacts known credentials/tokens.
pub fn redact_text(input: &str) -> String {
    let mut current: Cow<'_, str> = Cow::Borrowed(input);
    current = apply_replace(current, private_block_re(), "");
    current = apply_replace(current, pem_private_key_re(), "[REDACTED_PRIVATE_KEY]");
    current = apply_replace(current, conn_password_re(), "${1}[REDACTED]${3}");
    current = apply_replace(current, bearer_re(), "Bearer [REDACTED_TOKEN]");
    current = apply_replace(current, jwt_re(), "[REDACTED_JWT]");
    current = apply_replace(current, github_token_re(), "[REDACTED_GITHUB_TOKEN]");
    current = apply_replace(current, aws_key_re(), "[REDACTED_AWS_KEY]");
    current = apply_replace(current, gcp_key_re(), "[REDACTED_GCP_KEY]");
    current = apply_replace(current, stripe_key_re(), "[REDACTED_STRIPE_KEY]");
    current = apply_replace(current, slack_token_re(), "[REDACTED_SLACK_TOKEN]");
    current = apply_replace(current, api_key_prefix_re(), "[REDACTED_API_KEY]");
    current = apply_replace(current, secret_kv_re(), "${1}${2}[REDACTED]${4}");
    current.into_owned()
}

/// Redacts all free-text fields in an `ObservationInput`.
pub fn redact_input(input: &ObservationInput) -> ObservationInput {
    ObservationInput {
        topic_key: input.topic_key.as_deref().map(redact_text),
        kind: input.kind,
        title: redact_text(&input.title),
        what: redact_text(&input.what),
        why: redact_text(&input.why),
        location: redact_text(&input.location),
        learned: redact_text(&input.learned),
        related_files: input.related_files.clone(),
        related_symbols: input.related_symbols.clone(),
        ttl_seconds: input.ttl_seconds,
    }
}

/// Redacts all free-text fields in a `MemoryChunkRecord` before ingestion.
pub fn redact_record(record: &MemoryChunkRecord) -> MemoryChunkRecord {
    MemoryChunkRecord {
        id: record.id.clone(),
        topic_key: redact_text(&record.topic_key),
        kind: record.kind,
        title: redact_text(&record.title),
        what: redact_text(&record.what),
        why: redact_text(&record.why),
        location: redact_text(&record.location),
        learned: redact_text(&record.learned),
        related_files: record.related_files.clone(),
        related_symbols: record.related_symbols.clone(),
        author: record.author.clone(),
        created_at: record.created_at,
        updated_at: record.updated_at,
        expires_at: record.expires_at,
    }
}
