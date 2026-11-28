//! OSV.dev batch vulnerability querying (`POST /v1/querybatch`) and response parsing.

use std::io::Read;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::http::ANVIL_DEPS_USER_AGENT;
use crate::model::{DependencySpec, Ecosystem};

const OSV_BATCH_URL: &str = "https://api.osv.dev/v1/querybatch";
const OSV_VULN_URL: &str = "https://api.osv.dev/v1/vulns/";
/// Only RustSec records carry `database_specific.informational`; other sources are always real vulnerabilities.
const INFORMATIONAL_ID_PREFIX: &str = "RUSTSEC-";
const OSV_TIMEOUT_SECS: u64 = 5;
const MAX_OSV_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CLEAN_VERSION_LEN: usize = 40;

#[derive(Serialize)]
struct OsvBatchRequest<'a> {
    queries: Vec<OsvQuery<'a>>,
}

#[derive(Serialize)]
struct OsvQuery<'a> {
    version: String,
    package: OsvPackage<'a>,
}

#[derive(Serialize)]
struct OsvPackage<'a> {
    name: &'a str,
    ecosystem: &'static str,
}

#[derive(Deserialize)]
struct OsvBatchResponse {
    #[serde(default)]
    results: Option<Vec<OsvBatchResult>>,
}

#[derive(Deserialize)]
struct OsvBatchResult {
    #[serde(default)]
    vulns: Option<Vec<OsvVuln>>,
}

#[derive(Deserialize)]
struct OsvVuln {
    id: String,
}

fn ecosystem_to_osv(ecosystem: Ecosystem) -> &'static str {
    match ecosystem {
        Ecosystem::Npm => "npm",
        Ecosystem::Pypi => "PyPI",
        Ecosystem::PubDev => "Pub",
        Ecosystem::Maven => "Maven",
        Ecosystem::Cargo => "crates.io",
        Ecosystem::Composer => "Packagist",
    }
}

/// Normalizes a manifest version requirement into a bare version string suitable for OSV queries.
pub(crate) fn clean_version_for_osv(version_req: Option<&str>) -> Option<String> {
    let mut raw = version_req?.trim();
    if raw.starts_with('[')
        && let Some(close_idx) = raw.find(']')
    {
        raw = raw[close_idx + 1..].trim();
    }
    if raw.is_empty() || raw.contains("||") || raw.contains('*') {
        return None;
    }

    let first_token =
        raw.split(|c: char| c == ',' || c.is_whitespace()).find(|seg| !seg.is_empty())?;
    let cleaned = first_token.trim_start_matches(['^', '~', '=', '>', '<', 'v']);

    if cleaned.is_empty()
        || cleaned.len() > MAX_CLEAN_VERSION_LEN
        || !cleaned.as_bytes().first().is_some_and(u8::is_ascii_digit)
    {
        return None;
    }

    Some(cleaned.to_owned())
}

/// Picks the version sent to OSV: the lockfile-resolved one when known, else the cleaned requirement.
pub(crate) fn osv_query_version(spec: &DependencySpec) -> Option<String> {
    match spec.resolved_version.as_deref() {
        Some(exact) => Some(exact.to_owned()),
        None => clean_version_for_osv(spec.version_req.as_deref()),
    }
}

/// Parses an OSV `/v1/querybatch` JSON payload into per-query lists of vulnerability IDs.
pub(crate) fn parse_osv_batch_response(raw_json: &str) -> Result<Vec<Vec<String>>> {
    let parsed: OsvBatchResponse =
        serde_json::from_str(raw_json).context("invalid OSV querybatch JSON response")?;
    let results = parsed.results.unwrap_or_default();
    Ok(results
        .into_iter()
        .map(|entry| {
            entry
                .vulns
                .unwrap_or_default()
                .into_iter()
                .map(|v| v.id)
                .filter(|id| !id.is_empty())
                .collect()
        })
        .collect())
}

fn post_osv_batch_json(body: &str) -> Result<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(OSV_TIMEOUT_SECS))
        .timeout_read(Duration::from_secs(OSV_TIMEOUT_SECS))
        .user_agent(ANVIL_DEPS_USER_AGENT)
        .build();

    let response = agent
        .post(OSV_BATCH_URL)
        .set("Content-Type", "application/json")
        .send_string(body)
        .context("OSV querybatch POST failed")?;

    let mut buf = Vec::new();
    let mut reader = response.into_reader().take(MAX_OSV_RESPONSE_BYTES + 1);
    reader.read_to_end(&mut buf).context("failed to read OSV querybatch response")?;
    if buf.len() as u64 > MAX_OSV_RESPONSE_BYTES {
        bail!("OSV querybatch response exceeded {MAX_OSV_RESPONSE_BYTES} byte safety cap");
    }
    String::from_utf8(buf).context("OSV querybatch response was not valid UTF-8")
}

/// Returns true when an OSV vulnerability record is a RustSec informational advisory
/// (`affected[].database_specific.informational` of `unmaintained`, `unsound`, or `notice`).
pub(crate) fn is_informational_record(raw_json: &str) -> bool {
    let Ok(record) = serde_json::from_str::<serde_json::Value>(raw_json) else {
        return false;
    };
    record
        .get("affected")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("database_specific")?.get("informational")?.as_str())
        .any(|kind| !kind.trim().is_empty())
}

fn get_osv_vuln_json(id: &str) -> Result<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(OSV_TIMEOUT_SECS))
        .timeout_read(Duration::from_secs(OSV_TIMEOUT_SECS))
        .user_agent(ANVIL_DEPS_USER_AGENT)
        .build();
    let response = agent
        .get(&format!("{OSV_VULN_URL}{}", crate::http::percent_encode_component(id)))
        .call()
        .context("OSV vulnerability GET failed")?;
    let mut buf = Vec::new();
    response
        .into_reader()
        .take(MAX_OSV_RESPONSE_BYTES)
        .read_to_end(&mut buf)
        .context("failed to read OSV vulnerability record")?;
    String::from_utf8(buf).context("OSV vulnerability record was not valid UTF-8")
}

/// Looks up which of `ids` are informational advisories; unreachable records count as real vulnerabilities.
pub(crate) fn fetch_informational_ids(ids: &[String]) -> FxHashSet<String> {
    informational_ids_with(ids, get_osv_vuln_json)
}

/// Same as [`fetch_informational_ids`] with an injectable record fetcher so tests stay offline.
pub(crate) fn informational_ids_with(
    ids: &[String],
    get: impl Fn(&str) -> Result<String>,
) -> FxHashSet<String> {
    ids.iter()
        .filter(|id| id.starts_with(INFORMATIONAL_ID_PREFIX))
        .filter(|id| get(id).is_ok_and(|raw| is_informational_record(&raw)))
        .cloned()
        .collect()
}

/// Queries OSV.dev in a single batch request and maps returned vulnerability IDs back to `specs` indices.
pub(crate) fn query_osv_batch(specs: &[DependencySpec]) -> FxHashMap<usize, Vec<String>> {
    query_osv_batch_with(specs, post_osv_batch_json)
}

/// Same as [`query_osv_batch`] but with an injectable transport so tests stay offline.
pub(crate) fn query_osv_batch_with(
    specs: &[DependencySpec],
    post: impl FnOnce(&str) -> Result<String>,
) -> FxHashMap<usize, Vec<String>> {
    let mut orig_indices = Vec::new();
    let mut queries = Vec::new();

    for (idx, spec) in specs.iter().enumerate() {
        if let Some(version) = osv_query_version(spec) {
            orig_indices.push(idx);
            queries.push(OsvQuery {
                version,
                package: OsvPackage {
                    name: &spec.name,
                    ecosystem: ecosystem_to_osv(spec.ecosystem),
                },
            });
        }
    }

    if queries.is_empty() {
        return FxHashMap::default();
    }

    let Ok(body) = serde_json::to_string(&OsvBatchRequest { queries }) else {
        return FxHashMap::default();
    };
    let Ok(raw_json) = post(&body) else {
        return FxHashMap::default();
    };
    let Ok(parsed) = parse_osv_batch_response(&raw_json) else {
        return FxHashMap::default();
    };

    let mut out = FxHashMap::default();
    for (spec_idx, vulns) in orig_indices.into_iter().zip(parsed) {
        if !vulns.is_empty() {
            out.insert(spec_idx, vulns);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_valid_version_specs_and_rejects_ranges_or_wildcards() {
        assert_eq!(clean_version_for_osv(Some("^1.2.3")), Some("1.2.3".to_owned()));
        assert_eq!(clean_version_for_osv(Some("~=2.9.1")), Some("2.9.1".to_owned()));
        assert_eq!(clean_version_for_osv(Some(">=0.115.0, <1.0.0")), Some("0.115.0".to_owned()));
        assert_eq!(clean_version_for_osv(Some("  v12.4.0  ")), Some("12.4.0".to_owned()));
        assert_eq!(clean_version_for_osv(Some("==2.32.3")), Some("2.32.3".to_owned()));
        assert_eq!(clean_version_for_osv(Some("[standard]>=0.30.0")), Some("0.30.0".to_owned()));

        assert_eq!(clean_version_for_osv(None), None);
        assert_eq!(clean_version_for_osv(Some("")), None);
        assert_eq!(clean_version_for_osv(Some("1.0.0 || 2.0.0")), None);
        assert_eq!(clean_version_for_osv(Some("1.*")), None);
        assert_eq!(clean_version_for_osv(Some("*")), None);
        assert_eq!(clean_version_for_osv(Some("latest")), None);
        assert_eq!(clean_version_for_osv(Some(&"1".repeat(MAX_CLEAN_VERSION_LEN + 1))), None);
    }

    #[test]
    fn parses_osv_batch_response_with_empty_and_populated_entries() {
        let payload = r#"{
          "results": [
            {},
            { "vulns": [ { "id": "GHSA-35jh-r3h4-6jhm" }, { "id": "CVE-2021-23337" } ] },
            { "vulns": null },
            { "vulns": [ { "id": "RUSTSEC-2024-0001" } ] }
          ]
        }"#;
        let parsed = parse_osv_batch_response(payload).expect("valid osv batch json");
        assert_eq!(
            parsed,
            vec![
                Vec::<String>::new(),
                vec!["GHSA-35jh-r3h4-6jhm".to_owned(), "CVE-2021-23337".to_owned()],
                Vec::<String>::new(),
                vec!["RUSTSEC-2024-0001".to_owned()],
            ]
        );
        assert!(parse_osv_batch_response("not json").is_err());
    }

    #[test]
    fn detects_rustsec_informational_records() {
        let unmaintained = r#"{ "id": "RUSTSEC-2026-0543", "affected": [
          { "package": { "name": "chrono" }, "database_specific": { "informational": "unmaintained" } } ] }"#;
        let unsound =
            r#"{ "affected": [ { "database_specific": { "informational": "unsound" } } ] }"#;
        let real =
            r#"{ "affected": [ { "database_specific": { "cvss": null, "categories": [] } } ] }"#;
        assert!(is_informational_record(unmaintained));
        assert!(is_informational_record(unsound));
        assert!(!is_informational_record(real));
        assert!(!is_informational_record("{}"));
        assert!(!is_informational_record("not json"));
    }

    #[test]
    fn only_rustsec_ids_are_looked_up_and_failures_stay_vulnerable() {
        let ids = vec![
            "RUSTSEC-2026-0543".to_owned(),
            "RUSTSEC-2022-0013".to_owned(),
            "RUSTSEC-2099-0001".to_owned(),
            "GHSA-m5pq-gvj9-9vr8".to_owned(),
        ];
        let lookups = std::cell::RefCell::new(Vec::new());
        let found = informational_ids_with(&ids, |id| {
            lookups.borrow_mut().push(id.to_owned());
            match id {
                "RUSTSEC-2026-0543" => Ok(r#"{ "affected": [ { "database_specific": { "informational": "unmaintained" } } ] }"#.to_owned()),
                "RUSTSEC-2022-0013" => Ok(r#"{ "affected": [ {} ] }"#.to_owned()),
                _ => anyhow::bail!("offline"),
            }
        });
        assert_eq!(found.len(), 1);
        assert!(found.contains("RUSTSEC-2026-0543"));
        assert!(!lookups.borrow().iter().any(|id| id.starts_with("GHSA")));
    }

    #[test]
    fn maps_ecosystems_to_osv_names() {
        assert_eq!(ecosystem_to_osv(Ecosystem::Npm), "npm");
        assert_eq!(ecosystem_to_osv(Ecosystem::Pypi), "PyPI");
        assert_eq!(ecosystem_to_osv(Ecosystem::PubDev), "Pub");
        assert_eq!(ecosystem_to_osv(Ecosystem::Maven), "Maven");
        assert_eq!(ecosystem_to_osv(Ecosystem::Cargo), "crates.io");
        assert_eq!(ecosystem_to_osv(Ecosystem::Composer), "Packagist");
    }
}
