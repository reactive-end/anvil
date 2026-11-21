//! Lightweight synchronous HTTP and timestamp utilities shared across ecosystems.

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

const HTTP_TIMEOUT_SECS: u64 = 5;

fn shared_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .timeout_read(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .user_agent("anvil-deps/0.1")
            .build()
    })
}

/// Fetches a JSON payload synchronously via `ureq` with short timeouts.
pub(crate) fn fetch_json_text(url: &str) -> Result<String> {
    let response = shared_agent()
        .get(url)
        .set("Accept", "application/json")
        .call()
        .with_context(|| format!("HTTP GET failed for {url}"))?;
    response
        .into_string()
        .with_context(|| format!("failed to read HTTP response body from {url}"))
}

/// Percent-encodes a URL path/query component (RFC 3986 unreserved characters preserved).
pub(crate) fn percent_encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// Parses an RFC 3339 / ISO 8601 timestamp into UTC.
pub(crate) fn parse_rfc3339_utc(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw.trim())
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}
