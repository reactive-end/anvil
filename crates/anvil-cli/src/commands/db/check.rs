//! `anvil db check`: is Toolbox installed, is the tools file sane, does a query go through.

use std::fs;
use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_qa::api::db::{DOWNLOAD_HINT, ToolboxClient, select};
use anyhow::Result;

use crate::commands::ExitCodeError;
use crate::ui;

/// Words in a key that mark a credential.
const SECRET_KEYS: [&str; 5] = ["password", "passwd", "secret", "token", "api_key"];

pub(super) fn run(root: &Path) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    let client = ToolboxClient::new(root, &config.db);
    let mut failed = false;

    let binary_ok = match client.version() {
        Ok(version) => {
            ui::print_status_row(&ui::ok_badge(), "toolbox", &version);
            true
        }
        Err(err) => {
            let detail = format!("{err:#} (download: {DOWNLOAD_HINT})");
            ui::print_status_row(&ui::fail_badge(), "toolbox", &detail);
            false
        }
    };
    failed |= !binary_ok;

    let file_ok = match fs::read_to_string(client.config()) {
        Ok(text) => {
            ui::print_status_row(&ui::ok_badge(), "config", &config.db.config);
            failed |= report_literal_secrets(&text);
            true
        }
        Err(err) => {
            let detail =
                format!("{} cannot be read ({err}); run `anvil db init`", config.db.config);
            ui::print_status_row(&ui::fail_badge(), "config", &detail);
            false
        }
    };
    failed |= !file_ok;

    if binary_ok && file_ok {
        failed |= !probe(&client);
    }
    if failed { Err(ExitCodeError::silent(1).into()) } else { Ok(()) }
}

/// Prints one row per literal credential; returns `true` when there are any.
fn report_literal_secrets(text: &str) -> bool {
    let found = literal_secrets(text);
    for (line, key) in &found {
        let detail = format!(
            "line {line}: `{key}` holds a literal value; use an environment variable placeholder"
        );
        ui::print_status_row(&ui::fail_badge(), "secrets", &detail);
    }
    !found.is_empty()
}

fn probe(client: &ToolboxClient) -> bool {
    match select(client, "SELECT 1") {
        Ok(rows) => {
            ui::print_status_row(
                &ui::ok_badge(),
                "query",
                &format!("SELECT 1 through anvil_select returned {} row(s)", rows.len()),
            );
            true
        }
        Err(err) => {
            ui::print_status_row(
                &ui::fail_badge(),
                "query",
                &format!("SELECT 1 through anvil_select failed: {err:#}"),
            );
            false
        }
    }
}

/// `(line number, key)` of every credential-like key whose value is not a `${...}` placeholder.
/// Values are never returned, so they cannot be printed by accident.
pub(super) fn literal_secrets(text: &str) -> Vec<(usize, String)> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once(':')?;
            let key = key.trim().trim_start_matches("- ");
            let lower = key.to_ascii_lowercase();
            if !SECRET_KEYS.iter().any(|word| lower.contains(word)) {
                return None;
            }
            let value = value.split(" #").next().unwrap_or_default().trim();
            let value = value.trim_matches(|c| c == '"' || c == '\'');
            (!value.is_empty() && !value.starts_with("${")).then(|| (index + 1, key.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_literal_credentials_are_flagged_and_never_their_values() {
        let yaml = "\
kind: source
name: app-db
password: ${DB_PASSWORD}
user: ${DB_USER}
---
# password: commented-out
kind: source
password: \"hunter2\"
client_secret: abc
api_key:
auth_token: ${TOKEN}
description: how passwords are stored
";
        let found = literal_secrets(yaml);
        assert_eq!(found, vec![(8, "password".to_owned()), (9, "client_secret".to_owned())]);
    }

    #[test]
    fn a_generated_file_has_no_literal_credentials() {
        let yaml = "kind: source\ntype: postgres\nhost: ${DB_HOST}\npassword: ${DB_PASSWORD}\n";
        assert!(literal_secrets(yaml).is_empty());
    }
}
