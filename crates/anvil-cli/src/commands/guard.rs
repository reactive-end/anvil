//! `anvil guard`: inspect and configure the destructive-command security guard.

use std::fs;
use std::path::Path;

use anvil_adapters::claude_code::config as claude_config;
use anvil_adapters::contract::{AdapterKind, HookEvent, NormalizedHookInput};
use anvil_core::config::AnvilConfig;
use anvil_core::config::security_guard::{
    GuardCategory, GuardDecision, PatternList, SecurityGuardConfig, check_pattern,
};
use anvil_core::fs_util::atomic_write;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::cli::subcmds::GuardSubcmd;
use crate::commands::hook::security_guard;

const NO_RISK: &str = "no destructive command or secret access found";

/// Executes `anvil guard list|set|add|remove|reset|test`.
pub fn execute(root: &Path, sub: GuardSubcmd) -> Result<()> {
    match sub {
        GuardSubcmd::List => list(root),
        GuardSubcmd::Test { command, tool } => test(root, &command, tool.as_deref()),
        GuardSubcmd::Set { category, decision } => {
            let category = GuardCategory::parse(&category)?;
            let decision = GuardDecision::parse(&decision)?;
            update(root, |guard| {
                guard.categories.set(category, decision);
                Ok(())
            })?;
            println!("security guard: `{}` -> {}", category.as_str(), decision.as_str());
            Ok(())
        }
        GuardSubcmd::Add { decision, pattern } => {
            let decision = GuardDecision::parse(&decision)?;
            check_pattern(&pattern)?;
            update(root, |guard| {
                let list = guard.patterns_mut(PatternList::for_decision(decision));
                if !list.contains(&pattern) {
                    list.push(pattern.clone());
                }
                Ok(())
            })?;
            println!("security guard: commands matching `{pattern}` -> {}", decision.as_str());
            Ok(())
        }
        GuardSubcmd::Remove { pattern } => {
            update(root, |guard| remove_pattern(guard, &pattern))?;
            println!("security guard: removed pattern `{pattern}`");
            Ok(())
        }
        GuardSubcmd::Reset => {
            update(root, |guard| {
                *guard = SecurityGuardConfig::default();
                Ok(())
            })?;
            println!("security guard: defaults restored");
            Ok(())
        }
    }
}

fn remove_pattern(guard: &mut SecurityGuardConfig, pattern: &str) -> Result<()> {
    let mut removed = false;
    for list in [PatternList::Deny, PatternList::Ask, PatternList::Allow] {
        let patterns = guard.patterns_mut(list);
        let before = patterns.len();
        patterns.retain(|p| p != pattern);
        removed |= patterns.len() != before;
    }
    if !removed {
        bail!("no pattern `{pattern}` is configured (see `anvil guard list`)");
    }
    Ok(())
}

fn list(root: &Path) -> Result<()> {
    let guard = AnvilConfig::load_or_default(root)?.security_guard;
    println!("security guard decisions:");
    for category in GuardCategory::ALL {
        let decision = guard.decision(category);
        let default = category.default_decision();
        let tail = if decision == default {
            String::new()
        } else {
            format!("  (default: {})", default.as_str())
        };
        println!("  {:<11} {}{tail}", category.as_str(), decision.as_str());
    }
    let invalid = guard.invalid_patterns();
    for list in [PatternList::Deny, PatternList::Ask, PatternList::Allow] {
        let patterns = guard.patterns(list);
        println!("{}: {}", list.key(), if patterns.is_empty() { "(none)" } else { "" });
        for pattern in patterns {
            match invalid.iter().find(|i| i.list == list && i.pattern == *pattern) {
                Some(bad) => println!("  {pattern}  [INVALID regex, ignored: {}]", bad.error),
                None => println!("  {pattern}"),
            }
        }
    }
    println!("precedence: deny_patterns > allow_patterns > ask_patterns > category decision");
    Ok(())
}

fn test(root: &Path, command: &str, tool: Option<&str>) -> Result<()> {
    let guard = AnvilConfig::load_or_default(root)?.security_guard;
    let tool_name = tool.unwrap_or("Bash");
    let input = NormalizedHookInput {
        adapter: AdapterKind::ClaudeCode,
        event: HookEvent::PreToolUse,
        cwd: None,
        session_id: None,
        agent_id: None,
        agent_type: None,
        transcript_path: None,
        tool_use_id: None,
        tool_name: Some(tool_name.to_owned()),
        tool_input: Some(security_guard::tool_input_for(tool_name, command)),
        tool_output: None,
        prompt: None,
        user_prompt: None,
    };
    let verdict = security_guard::evaluate(root, &guard, &input);
    println!("decision: {}", verdict.decision.as_str());
    println!("category: {}", verdict.category.map_or("-", GuardCategory::as_str));
    let reason = if verdict.reason.is_empty() { NO_RISK } else { verdict.reason.as_str() };
    println!("reason: {reason}");
    Ok(())
}

/// Applies `change` to the `security_guard` section of `.harness/config.json`, keeping every other
/// key, then refreshes the Claude Code permission rules derived from it.
fn update(
    root: &Path,
    change: impl FnOnce(&mut SecurityGuardConfig) -> Result<()>,
) -> Result<SecurityGuardConfig> {
    let path = AnvilConfig::path(root);
    let mut doc = if path.exists() {
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str::<Value>(raw.strip_prefix('\u{feff}').unwrap_or(&raw))
            .with_context(|| format!("failed to parse {}", path.display()))?
    } else {
        json!({})
    };
    let fields = doc.as_object_mut().context("`.harness/config.json` must hold a JSON object")?;
    let mut guard = match fields.get("security_guard") {
        Some(section) => serde_json::from_value(section.clone())
            .context("invalid `security_guard` section in `.harness/config.json`")?,
        None => SecurityGuardConfig::default(),
    };
    change(&mut guard)?;
    fields.insert("security_guard".to_owned(), serde_json::to_value(&guard)?);
    let json = serde_json::to_string_pretty(&doc).context("failed to serialize config")?;
    atomic_write(&path, format!("{json}\n").as_bytes())?;
    sync_permissions(root, &guard)?;
    Ok(guard)
}

fn sync_permissions(root: &Path, guard: &SecurityGuardConfig) -> Result<()> {
    if !claude_config::settings_config_path(root).exists() {
        return Ok(());
    }
    if claude_config::merge_settings_with_guard(root, guard)? {
        println!("updated .claude/settings.json permissions");
    }
    Ok(())
}
