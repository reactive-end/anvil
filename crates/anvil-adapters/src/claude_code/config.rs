//! Non-destructive generator and merger for `.claude/settings.json` and `.mcp.json`.

use std::path::{Path, PathBuf};

use anvil_core::config::AnvilConfig;
use anvil_core::config::security_guard::{GuardCategory, GuardDecision, SecurityGuardConfig};
use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use crate::antigravity::config::{
    ensure_array_field, ensure_object_field, merge_mcp_servers_json,
    merge_umbrella_mcp_servers_json, read_json_object,
};
use crate::contract::HookEvent;
use crate::sync::write_if_changed;

const CLAUDE_EVENTS: &[HookEvent] = &[
    HookEvent::SessionStart,
    HookEvent::UserPromptSubmit,
    HookEvent::PreToolUse,
    HookEvent::PostToolUse,
    HookEvent::Stop,
];

/// Returns the path to `.claude/settings.json`.
pub fn settings_config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".claude").join("settings.json")
}

/// Returns the path to root `.mcp.json`.
pub fn mcp_config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".mcp.json")
}

/// Permission rules Anvil owns, keyed by the guard category that decides their list. Each entry is
/// the command prefix of a `Bash(<prefix>:*)` rule.
const MANAGED_RULES: &[(GuardCategory, &[&str])] = &[
    (
        GuardCategory::Cloud,
        &[
            "aws ec2 terminate-instances",
            "aws ec2 delete-vpc",
            "aws ec2 delete-volume",
            "aws s3 rb",
            "aws s3 rm",
            "aws s3api delete-bucket",
            "aws rds delete-db-instance",
            "aws rds delete-db-cluster",
            "aws cloudformation delete-stack",
            "aws eks delete-cluster",
            "aws lambda delete-function",
            "aws dynamodb delete-table",
            "aws iam delete-user",
            "aws iam delete-role",
            "gcloud projects delete",
            "gcloud compute instances delete",
            "gcloud sql instances delete",
            "gcloud container clusters delete",
            "gcloud storage rm",
            "gcloud storage buckets delete",
            "gsutil rm",
            "gsutil rb",
            "az group delete",
            "az vm delete",
            "az keyvault purge",
            "az storage account delete",
        ],
    ),
    (
        GuardCategory::Infra,
        &[
            "terraform destroy",
            "terraform apply -destroy",
            "terraform apply -auto-approve",
            "terraform state rm",
            "terraform workspace delete",
            "terraform force-unlock",
            "tofu destroy",
            "tofu apply -destroy",
            "tofu apply -auto-approve",
            "tofu state rm",
            "terragrunt destroy",
            "terragrunt run-all destroy",
            "pulumi destroy",
            "pulumi stack rm",
            "cdk destroy",
            "serverless remove",
            "sls remove",
            "sam delete",
            "kubectl delete",
            "kubectl drain",
            "helm uninstall",
            "helm delete",
        ],
    ),
    (
        GuardCategory::Containers,
        &[
            "docker system prune",
            "docker volume prune",
            "docker volume rm",
            "docker container prune",
            "docker image prune",
            "docker compose down -v",
            "docker-compose down -v",
            "podman system prune",
            "podman volume prune",
            "podman volume rm",
        ],
    ),
];

/// Non-destructively merges `anvil hook` handlers and the guard permission rules into
/// `.claude/settings.json`, using the `security_guard` section of `.harness/config.json`.
pub fn merge_settings_config(repo_root: &Path) -> Result<bool> {
    let guard = AnvilConfig::load_or_default(repo_root)?.security_guard;
    merge_settings_with_guard(repo_root, &guard)
}

/// Like [`merge_settings_config`], for an already loaded guard configuration.
pub fn merge_settings_with_guard(repo_root: &Path, guard: &SecurityGuardConfig) -> Result<bool> {
    let path = settings_config_path(repo_root);
    let mut root = read_json_object(&path)?;

    let hooks_obj = ensure_object_field(&mut root, "hooks")?;
    for event in CLAUDE_EVENTS {
        let key = event.as_pascal_case();
        let cli_event = match event {
            HookEvent::UserPromptSubmit => "user-prompt-submit",
            _ => key,
        };
        let command = format!("anvil hook {cli_event} --adapter claude");
        let use_matcher = matches!(event, HookEvent::PreToolUse | HookEvent::PostToolUse);
        let list = ensure_array_field(hooks_obj, key)?;
        upsert_claude_hook_entry(list, &command, use_matcher);
    }
    sync_guard_permissions(&mut root, guard)?;

    let formatted = serde_json::to_string_pretty(&Value::Object(root))
        .context("failed to serialize .claude/settings.json")?;
    write_if_changed(&path, &format!("{formatted}\n"))
}

/// Non-destructively merges the `anvil mcp` server entry into root `.mcp.json`.
pub fn merge_mcp_config(repo_root: &Path) -> Result<bool> {
    merge_mcp_servers_json(&mcp_config_path(repo_root), ".mcp.json")
}

/// Non-destructively merges per-subrepo `anvil-<slug>` MCP entries into `.mcp.json` for Thin Umbrella roots.
pub fn merge_umbrella_mcp_config(root: &Path, subrepos: &[(String, PathBuf)]) -> Result<bool> {
    merge_umbrella_mcp_servers_json(&mcp_config_path(root), ".mcp.json", subrepos)
}

fn upsert_claude_hook_entry(entries: &mut Vec<Value>, command: &str, use_matcher: bool) {
    let hook_item = json!({
        "type": "command",
        "command": command
    });
    if let Some(existing) = entries.iter_mut().find(|e| is_anvil_claude_entry(e))
        && let Some(arr) = existing.get_mut("hooks").and_then(Value::as_array_mut)
        && let Some(item) = arr.iter_mut().find(|h| is_anvil_hook_item(h))
    {
        *item = hook_item;
        return;
    }
    let new_entry = if use_matcher {
        json!({ "matcher": "*", "hooks": [hook_item] })
    } else {
        json!({ "hooks": [hook_item] })
    };
    entries.push(new_entry);
}

fn is_anvil_claude_entry(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|arr| arr.iter().any(is_anvil_hook_item))
}

fn is_anvil_hook_item(item: &Value) -> bool {
    item.get("command").and_then(Value::as_str).is_some_and(|cmd| cmd.starts_with("anvil hook "))
}

/// Returns the Anvil-managed `(deny, ask)` rules the guard configuration requires.
pub fn guard_permission_rules(guard: &SecurityGuardConfig) -> (Vec<String>, Vec<String>) {
    let (mut deny, mut ask) = (Vec::new(), Vec::new());
    for (category, prefixes) in MANAGED_RULES {
        let target = match guard.decision(*category) {
            GuardDecision::Deny => &mut deny,
            GuardDecision::Ask => &mut ask,
            GuardDecision::Allow => continue,
        };
        target.extend(prefixes.iter().map(|prefix| format!("Bash({prefix}:*)")));
    }
    (deny, ask)
}

fn is_managed_rule(rule: &str) -> bool {
    let prefix = rule.strip_prefix("Bash(").and_then(|r| r.strip_suffix(":*)"));
    prefix.is_some_and(|p| MANAGED_RULES.iter().any(|(_, prefixes)| prefixes.contains(&p)))
}

/// Replaces the managed rules in `permissions.deny` / `permissions.ask` with the ones the guard
/// requires; user-written rules and every other key stay untouched.
fn sync_guard_permissions(
    root: &mut Map<String, Value>,
    guard: &SecurityGuardConfig,
) -> Result<()> {
    let (deny, ask) = guard_permission_rules(guard);
    if !root.contains_key("permissions") && deny.is_empty() && ask.is_empty() {
        return Ok(());
    }
    let permissions = ensure_object_field(root, "permissions")?;
    let emptied_deny = sync_rule_list(permissions, "deny", &deny)?;
    let emptied_ask = sync_rule_list(permissions, "ask", &ask)?;
    if (emptied_deny || emptied_ask) && permissions.is_empty() {
        root.remove("permissions");
    }
    Ok(())
}

/// Returns `true` when the list held only managed rules and was removed because none are required.
fn sync_rule_list(
    permissions: &mut Map<String, Value>,
    key: &str,
    wanted: &[String],
) -> Result<bool> {
    if wanted.is_empty() && !permissions.contains_key(key) {
        return Ok(false);
    }
    let list = ensure_array_field(permissions, key)?;
    let before = list.len();
    list.retain(|rule| !rule.as_str().is_some_and(is_managed_rule));
    let removed = before - list.len();
    list.extend(wanted.iter().cloned().map(Value::String));
    if list.is_empty() && removed > 0 {
        permissions.remove(key);
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn read(root: &Path) -> Value {
        let raw = std::fs::read_to_string(settings_config_path(root)).expect("read settings");
        serde_json::from_str(&raw).expect("parse settings")
    }

    fn rules(settings: &Value, list: &str) -> Vec<String> {
        settings["permissions"][list]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default()
    }

    #[test]
    fn default_guard_denies_cloud_and_infra_and_asks_for_containers() {
        let (deny, ask) = guard_permission_rules(&SecurityGuardConfig::default());
        for rule in [
            "Bash(aws ec2 terminate-instances:*)",
            "Bash(aws s3 rb:*)",
            "Bash(aws s3 rm:*)",
            "Bash(aws rds delete-db-instance:*)",
            "Bash(aws cloudformation delete-stack:*)",
            "Bash(gcloud projects delete:*)",
            "Bash(terraform destroy:*)",
            "Bash(tofu destroy:*)",
            "Bash(pulumi destroy:*)",
            "Bash(cdk destroy:*)",
            "Bash(kubectl delete:*)",
            "Bash(helm uninstall:*)",
        ] {
            assert!(deny.iter().any(|r| r == rule), "missing deny rule {rule}");
        }
        assert!(ask.iter().any(|r| r == "Bash(docker system prune:*)"));
        assert!(!deny.iter().any(|r| r.contains("docker")));
    }

    #[test]
    fn merge_keeps_user_rules_and_moves_managed_rules_with_the_decision() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join(".claude")).expect("mkdir");
        std::fs::write(
            settings_config_path(root),
            r#"{"model":"sonnet","permissions":{"allow":["Bash(npm test)"],"deny":["Bash(rm -rf:*)","Bash(aws s3 rb:*)"],"ask":["Bash(git push:*)"]}}"#,
        )
        .expect("seed");

        let mut guard = SecurityGuardConfig::default();
        assert!(merge_settings_with_guard(root, &guard).expect("merge"));
        let settings = read(root);
        assert_eq!(settings["model"], "sonnet");
        assert_eq!(settings["permissions"]["allow"], json!(["Bash(npm test)"]));
        let deny = rules(&settings, "deny");
        assert_eq!(deny[0], "Bash(rm -rf:*)");
        assert_eq!(deny.iter().filter(|r| *r == "Bash(aws s3 rb:*)").count(), 1);
        assert_eq!(rules(&settings, "ask")[0], "Bash(git push:*)");
        assert!(!merge_settings_with_guard(root, &guard).expect("idempotent merge"));

        guard.categories.set(GuardCategory::Cloud, GuardDecision::Ask);
        merge_settings_with_guard(root, &guard).expect("cloud ask");
        let settings = read(root);
        let deny = rules(&settings, "deny");
        assert!(!deny.iter().any(|r| r == "Bash(aws s3 rb:*)"));
        assert!(deny.iter().any(|r| r == "Bash(rm -rf:*)"));
        let ask = rules(&settings, "ask");
        assert!(ask.iter().any(|r| r == "Bash(aws s3 rb:*)"));
        assert_eq!(ask[0], "Bash(git push:*)");

        for category in GuardCategory::ALL {
            guard.categories.set(category, GuardDecision::Allow);
        }
        merge_settings_with_guard(root, &guard).expect("all allow");
        let settings = read(root);
        assert_eq!(settings["permissions"]["deny"], json!(["Bash(rm -rf:*)"]));
        assert_eq!(settings["permissions"]["ask"], json!(["Bash(git push:*)"]));
    }

    #[test]
    fn merge_removes_lists_and_permissions_it_created_when_everything_is_allowed() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let mut guard = SecurityGuardConfig::default();
        merge_settings_with_guard(root, &guard).expect("defaults");
        assert!(read(root)["permissions"]["deny"].is_array());
        for category in GuardCategory::ALL {
            guard.categories.set(category, GuardDecision::Allow);
        }
        merge_settings_with_guard(root, &guard).expect("all allow");
        assert!(read(root).get("permissions").is_none());
    }
}
