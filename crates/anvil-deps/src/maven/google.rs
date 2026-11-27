//! Google Maven (`dl.google.com/dl/android/maven2/.../maven-metadata.xml`) fallback fetcher.

use anyhow::{Result, bail};
use chrono::{DateTime, NaiveDateTime, Utc};

use crate::http::fetch_text_with_accept;
use crate::model::{Ecosystem, PackageHealthInfo};

/// Returns `true` for Android/Google Maven coordinates hosted primarily on `maven.google.com`.
pub fn is_google_maven_group(group: &str) -> bool {
    group == "androidx"
        || group.starts_with("androidx.")
        || group.starts_with("com.google.android.")
        || group.starts_with("com.android.")
}

/// Fetches `maven-metadata.xml` from Google's Maven repository for `group:artifact`.
pub(crate) fn fetch_google_maven(
    package: &str,
    group: &str,
    artifact: &str,
) -> Result<PackageHealthInfo> {
    let group_path = group.replace('.', "/");
    let url = format!(
        "https://dl.google.com/dl/android/maven2/{group_path}/{artifact}/maven-metadata.xml"
    );
    let xml = fetch_text_with_accept(&url, "application/xml, text/xml")?;
    parse_google_maven_metadata_xml(package, &xml)
}

/// Parses a Google Maven `maven-metadata.xml` payload into [`PackageHealthInfo`].
pub fn parse_google_maven_metadata_xml(package: &str, xml: &str) -> Result<PackageHealthInfo> {
    if !xml.contains("<metadata") {
        bail!("invalid Google Maven metadata XML for `{package}`");
    }

    let latest_version = extract_xml_tag(xml, "release")
        .or_else(|| extract_xml_tag(xml, "latest"))
        .or_else(|| extract_last_xml_tag(xml, "version"));

    let last_publish = extract_xml_tag(xml, "lastUpdated").and_then(|s| parse_maven_timestamp(&s));

    Ok(PackageHealthInfo {
        ecosystem: Ecosystem::Maven,
        name: package.to_owned(),
        latest_version,
        last_publish,
        deprecated: false,
        deprecation_message: None,
        archived: false,
        repository_url: Some("https://maven.google.com/".to_owned()),
        vulnerabilities: Vec::new(),
        informational: Vec::new(),
    })
}

fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let rest = &xml[start..];
    let end = rest.find(&close)?;
    let val = rest[..end].trim();
    (!val.is_empty()).then(|| val.to_owned())
}

fn extract_last_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let end_tag_pos = xml.rfind(&close)?;
    let prefix = &xml[..end_tag_pos];
    let start_tag_pos = prefix.rfind(&open)? + open.len();
    let val = prefix[start_tag_pos..].trim();
    (!val.is_empty()).then(|| val.to_owned())
}

fn parse_maven_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    let trimmed = raw.trim();
    if trimmed.len() == 14 {
        NaiveDateTime::parse_from_str(trimmed, "%Y%m%d%H%M%S").ok().map(|dt| dt.and_utc())
    } else if trimmed.len() == 8 {
        NaiveDateTime::parse_from_str(&format!("{trimmed}000000"), "%Y%m%d%H%M%S")
            .ok()
            .map(|dt| dt.and_utc())
    } else {
        None
    }
}
