//! Lightweight synchronous HTTP and timestamp utilities shared across ecosystems.

use std::io::Read;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};

const HTTP_TIMEOUT_SECS: u64 = 5;
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
const CRATES_IO_MIN_INTERVAL: Duration = Duration::from_secs(1);
pub(crate) const ANVIL_DEPS_USER_AGENT: &str =
    "anvil-deps/0.2 (+https://github.com/reactive-end/anvil)";

fn shared_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .timeout_read(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .user_agent(ANVIL_DEPS_USER_AGENT)
            .build()
    })
}

/// Computes remaining wait time before the next rate-limited request may proceed.
pub(crate) fn next_throttle_delay(
    last_request: Option<Instant>,
    now: Instant,
    min_interval: Duration,
) -> Option<Duration> {
    let prev = last_request?;
    let elapsed = now.saturating_duration_since(prev);
    min_interval.checked_sub(elapsed).filter(|d| !d.is_zero())
}

pub(crate) fn throttle_with(gate: &Mutex<Option<Instant>>, min_interval: Duration) {
    let mut guard = match gate.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(delay) = next_throttle_delay(*guard, Instant::now(), min_interval) {
        std::thread::sleep(delay);
    }
    *guard = Some(Instant::now());
}

/// Enforces `crates.io` crawler policy (`<= 1 req/s` sequentially) for live HTTP fetches.
pub(crate) fn fetch_crates_io_json(url: &str) -> Result<String> {
    static CRATES_IO_GATE: Mutex<Option<Instant>> = Mutex::new(None);
    throttle_with(&CRATES_IO_GATE, CRATES_IO_MIN_INTERVAL);
    fetch_json_text(url)
}

/// Fetches a JSON payload synchronously via `ureq` with short timeouts.
pub(crate) fn fetch_json_text(url: &str) -> Result<String> {
    fetch_text_with_accept(url, "application/json")
}

/// Fetches a text/JSON/XML payload synchronously with a custom `Accept` header and bounded body read.
pub(crate) fn fetch_text_with_accept(url: &str, accept: &str) -> Result<String> {
    let response = shared_agent()
        .get(url)
        .set("Accept", accept)
        .call()
        .with_context(|| format!("HTTP GET failed for {url}"))?;

    let mut buf = Vec::new();
    let mut reader = response.into_reader().take(MAX_RESPONSE_BYTES + 1);
    reader
        .read_to_end(&mut buf)
        .with_context(|| format!("failed to read HTTP response body from {url}"))?;
    if buf.len() as u64 > MAX_RESPONSE_BYTES {
        bail!("HTTP response from {url} exceeded {MAX_RESPONSE_BYTES} byte safety cap");
    }
    String::from_utf8(buf).with_context(|| format!("HTTP response from {url} was not valid UTF-8"))
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
    DateTime::parse_from_rfc3339(raw.trim()).ok().map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crates_io_user_agent_and_throttle_delay_calculation() {
        assert_eq!(
            ANVIL_DEPS_USER_AGENT,
            "anvil-deps/0.2 (+https://github.com/reactive-end/anvil)"
        );
        let t0 = Instant::now();
        assert_eq!(next_throttle_delay(None, t0, Duration::from_secs(1)), None);
        assert_eq!(
            next_throttle_delay(Some(t0), t0 + Duration::from_millis(250), Duration::from_secs(1)),
            Some(Duration::from_millis(750))
        );
        assert_eq!(
            next_throttle_delay(Some(t0), t0 + Duration::from_secs(1), Duration::from_secs(1)),
            None
        );
    }
}
