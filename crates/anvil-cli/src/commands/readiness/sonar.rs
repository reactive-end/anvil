//! Repositories that use SonarQube need it locally: the scanner on `PATH` and a server that answers.

use std::path::Path;
use std::time::Duration;

use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::resolve_sonar_bin;
use anvil_prereqs::{Item, Kind, Status};
use anvil_qa::flows::server::answers;

use crate::commands::sonar::resolve_sonar_endpoint;

const DEFAULT_HOST: &str = "http://localhost:9000";
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Scanner, server and token items when the root or a subrepo enables SonarQube.
pub fn items(root: &Path, config: &AnvilConfig) -> Vec<Item> {
    let users: Vec<Option<&str>> = std::iter::once(config.sonar.enabled.then_some(None))
        .chain(config.subrepos.iter().map(|s| s.sonar_enabled.then_some(Some(s.name.as_str()))))
        .flatten()
        .collect();
    let Some(&subrepo) = users.first() else {
        return Vec::new();
    };
    let (host, _, token) = resolve_sonar_endpoint(root, config, None, None);
    let host = host.unwrap_or_else(|| DEFAULT_HOST.to_owned());
    let scanner = match resolve_sonar_bin() {
        Some(bin) => Item::new(subrepo, Kind::Tool, "sonar-scanner", Status::Ok)
            .found(Some(bin.display().to_string())),
        None => Item::new(subrepo, Kind::Tool, "sonar-scanner", Status::Missing)
            .message("SonarQube is enabled but no scanner (`sonar` / `sonar-scanner`) is installed")
            .hint("`npm install -g sonarqube-scanner`, or set SONAR_SCANNER_BIN"),
    };
    let status_url = format!("{}/api/system/status", host.trim_end_matches('/'));
    let server = if answers(&status_url, PROBE_TIMEOUT) {
        Item::new(subrepo, Kind::Custom, "SonarQube server", Status::Ok).found(Some(host.clone()))
    } else {
        Item::new(subrepo, Kind::Custom, "SonarQube server", Status::Missing)
            .message(format!("no SonarQube answers at {host}"))
            .hint("start the local server (e.g. `docker run -d -p 9000:9000 sonarqube:community`) or set `sonar.host_url`")
    };
    let token_item = if token.is_some() {
        Item::new(subrepo, Kind::EnvVar, &config.sonar.token_env, Status::Ok)
    } else {
        Item::new(subrepo, Kind::EnvVar, &config.sonar.token_env, Status::Warning)
            .required(false)
            .message("no SonarQube token: analysis of private projects will be refused")
            .hint(&format!("set {} for this shell", config.sonar.token_env))
    };
    [scanner, server, token_item].into_iter().map(|item| item.source("sonar.enabled")).collect()
}
