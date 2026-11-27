use std::fs;
use std::path::Path;

use serde_json::Value;
use tempfile::tempdir;

use anvil_adapters::sync;
use anvil_adapters::templates;
use anvil_core::config::{AdapterTarget, AnvilConfig};

fn seed_existing_repo_fixtures(root: &Path) {
    fs::write(root.join("package.json"), r#"{"name":"fliiper-demo"}"#).expect("package.json");
    fs::write(root.join("tsconfig.json"), r#"{"compilerOptions":{"strict":true}}"#)
        .expect("tsconfig.json");

    let agy_custom = root.join(".agents/skills/fliiper-design-14");
    fs::create_dir_all(&agy_custom).expect("mkdir agy custom");
    fs::write(agy_custom.join("SKILL.md"), "# Fliiper Design 14\n").expect("agy skill");

    let claude_custom = root.join(".claude/skills/fliiper-qa-report");
    fs::create_dir_all(&claude_custom).expect("mkdir claude custom");
    fs::write(claude_custom.join("SKILL.md"), "# Fliiper QA Report\n").expect("claude skill");

    fs::write(
        root.join(".claude/settings.json"),
        r#"{"model":"sonnet","permissions":{"allow":["Bash(npm test)"]},"hooks":{"PreToolUse":[{"matcher":"Write","hooks":[{"type":"command","command":"custom-lint-guard"}]}]}}"#,
    )
    .expect("claude settings");
    fs::write(
        root.join(".agents/hooks.json"),
        r#"{"hooks":{"Stop":[{"command":"notify-slack"}]}}"#,
    )
    .expect("agy hooks");
    fs::write(
        root.join(".agents/mcp_config.json"),
        r#"{"mcpServers":{"playwright":{"command":"npx","args":["@playwright/mcp"]}}}"#,
    )
    .expect("agy mcp");
    fs::write(
        root.join(".mcp.json"),
        r#"{"mcpServers":{"github":{"command":"gh-mcp","args":[]}}}"#,
    )
    .expect("root mcp");
    fs::write(root.join("AGENTS.md"), "# Custom Header\nKeep this.\n").expect("AGENTS.md");
    fs::write(root.join(".gitignore"), "/target\n").expect(".gitignore");
}

#[test]
fn embedded_templates_meet_size_and_content_constraints() {
    assert!(templates::AGENTS_MD.lines().count() <= 34);
    assert!(templates::CLAUDE_MD.lines().count() <= 34);
    let required_tokens = [
        "<!-- anvil:managed:start -->",
        "<!-- anvil:managed:end -->",
        "Availability Guard",
        "PATH",
        "skip this entire block",
        "graph_outline",
        "graph_read_symbol",
        "anvil diff",
        "anvil grep",
        "anvil daemon start|stop|status",
        "anvil stats",
        "PreToolUse",
    ];
    for doc in [templates::AGENTS_MD, templates::CLAUDE_MD] {
        for token in required_tokens {
            assert!(doc.contains(token), "missing token: {token}");
        }
    }
    assert_eq!(templates::EMBEDDED_SKILLS.len(), 5);

    for skill in templates::EMBEDDED_SKILLS {
        assert!(skill.skill_md.contains(&format!("name: {}", skill.name)));
    }
}

#[test]
fn init_and_sync_preserve_preexisting_skills() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    seed_existing_repo_fixtures(root);

    let report = sync::init_repo(root).expect("init_repo");
    assert!(report.preserved_skills.contains(&"fliiper-design-14".to_owned()));
    assert!(report.preserved_skills.contains(&"fliiper-qa-report".to_owned()));

    assert_eq!(
        fs::read_to_string(root.join(".agents/skills/fliiper-design-14/SKILL.md"))
            .expect("read custom agy skill"),
        "# Fliiper Design 14\n"
    );
    assert_eq!(
        fs::read_to_string(root.join(".claude/skills/fliiper-qa-report/SKILL.md"))
            .expect("read custom claude skill"),
        "# Fliiper QA Report\n"
    );

    for skill in templates::EMBEDDED_SKILLS {
        assert!(!root.join(".harness/skills").join(skill.name).exists());
        assert!(root.join(".agents/skills").join(skill.name).join("SKILL.md").exists());
        assert!(root.join(".claude/skills").join(skill.name).join("SKILL.md").exists());
    }
}

#[test]
fn single_adapter_selection_skips_unused_adapter_and_avoids_duplication() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    fs::write(root.join("build.gradle.kts"), "plugins {}").expect("gradle");

    let custom = root.join(".harness/skills/android-custom/SKILL.md");
    fs::create_dir_all(custom.parent().unwrap()).expect("mkdir custom");
    fs::write(&custom, "# Custom Android Skill\n").expect("write custom skill");

    sync::init_repo_with_adapter(root, Some(anvil_core::config::AdapterTarget::Antigravity))
        .expect("init antigravity only");

    assert!(root.join(".agents/hooks.json").exists());
    assert!(root.join(".agents/skills/android-custom/SKILL.md").exists());
    assert!(root.join("AGENTS.md").exists());
    assert!(!root.join(".claude").exists());
    assert!(!root.join("CLAUDE.md").exists());
    assert!(!root.join(".mcp.json").exists());
}

#[test]
fn init_and_sync_update_docs_gitignore_config_idempotently() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    seed_existing_repo_fixtures(root);

    sync::init_repo(root).expect("init_repo");

    let agents_md = fs::read_to_string(root.join("AGENTS.md")).expect("read AGENTS.md");
    assert!(agents_md.contains("# Custom Header\nKeep this."));
    assert!(agents_md.contains("<!-- anvil:managed:start -->"));
    assert!(root.join("CLAUDE.md").exists());
    assert!(root.join(".harness/config.json").exists());

    let gitignore = fs::read_to_string(root.join(".gitignore")).expect("read .gitignore");
    assert!(gitignore.contains("/target"));
    assert!(gitignore.contains(".harness/.state/"));

    let second = sync::sync_repo(root).expect("second sync_repo");
    assert!(second.written_files.is_empty());
}

#[test]
fn init_merges_hooks_and_mcp_configs_without_clobbering_user_entries() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    seed_existing_repo_fixtures(root);
    sync::init_repo(root).expect("init_repo");

    let claude_settings: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".claude/settings.json")).expect("read settings"),
    )
    .expect("parse settings");
    assert_eq!(claude_settings["model"], "sonnet");
    assert_eq!(claude_settings["permissions"]["allow"][0], "Bash(npm test)");
    let pre_hooks = claude_settings["hooks"]["PreToolUse"].as_array().expect("PreToolUse array");
    assert_eq!(pre_hooks.len(), 2);
    assert_eq!(pre_hooks[0]["hooks"][0]["command"], "custom-lint-guard");
    assert_eq!(pre_hooks[1]["hooks"][0]["command"], "anvil hook PreToolUse --adapter claude");
    assert_eq!(
        claude_settings["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
        "anvil hook user-prompt-submit --adapter claude"
    );

    let agy_hooks: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".agents/hooks.json")).expect("read agy hooks"),
    )
    .expect("parse agy hooks");
    let stop_hooks = agy_hooks["hooks"]["Stop"].as_array().expect("Stop array");
    assert_eq!(stop_hooks.len(), 2);
    assert_eq!(stop_hooks[0]["command"], "notify-slack");
    assert_eq!(stop_hooks[1]["command"], "anvil hook Stop --adapter antigravity");

    let agy_mcp: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".agents/mcp_config.json")).expect("read agy mcp"),
    )
    .expect("parse agy mcp");
    assert_eq!(agy_mcp["mcpServers"]["playwright"]["command"], "npx");
    assert_eq!(agy_mcp["mcpServers"]["anvil"]["command"], "anvil");

    let claude_mcp: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".mcp.json")).expect("read .mcp.json"))
            .expect("parse .mcp.json");
    assert_eq!(claude_mcp["mcpServers"]["github"]["command"], "gh-mcp");
    assert_eq!(claude_mcp["mcpServers"]["anvil"]["command"], "anvil");
}

#[test]
fn sync_updates_only_managed_markdown_section_and_keeps_footer() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    fs::write(
        root.join("CLAUDE.md"),
        "# Top\n<!-- anvil:managed:start -->\nstale\n<!-- anvil:managed:end -->\n# Footer\n",
    )
    .expect("seed CLAUDE.md");

    sync::sync_repo(root).expect("sync_repo");
    let updated = fs::read_to_string(root.join("CLAUDE.md")).expect("read CLAUDE.md");
    assert!(updated.starts_with("# Top\n<!-- anvil:managed:start -->"));
    assert!(!updated.contains("\nstale\n"));
    assert!(updated.ends_with("<!-- anvil:managed:end -->\n# Footer\n"));
}

#[test]
fn cursor_adapter_syncs_mcp_rules_and_skills_idempotently() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    fs::create_dir_all(root.join(".cursor/rules")).expect("mkdir cursor rules");
    fs::write(
        root.join(".cursor/mcp.json"),
        r#"{"mcpServers":{"postgres":{"command":"pg-mcp","args":[]}}}"#,
    )
    .expect("seed cursor mcp");
    fs::write(
        root.join(".cursor/rules/anvil.mdc"),
        "---\ndescription: Custom\nglobs: *.rs\nalwaysApply: true\n---\n\n# Header\n<!-- anvil:managed:start -->\nstale\n<!-- anvil:managed:end -->\n# Footer\n",
    )
    .expect("seed cursor rules");

    sync::init_repo_with_adapter(root, Some(AdapterTarget::Cursor)).expect("init cursor");

    let mcp: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".cursor/mcp.json")).expect("read mcp"))
            .expect("parse cursor mcp");
    assert_eq!(mcp["mcpServers"]["postgres"]["command"], "pg-mcp");
    assert_eq!(mcp["mcpServers"]["anvil"]["command"], "anvil");
    assert_eq!(mcp["mcpServers"]["anvil"]["args"][0], "mcp");

    let mdc = fs::read_to_string(root.join(".cursor/rules/anvil.mdc")).expect("read mdc");
    assert!(mdc.contains("globs: *.rs"));
    assert!(mdc.contains("# Header\n<!-- anvil:managed:start -->"));
    assert!(!mdc.contains("\nstale\n"));
    assert!(mdc.ends_with("<!-- anvil:managed:end -->\n# Footer\n"));
    assert!(root.join(".cursor/skills/subagent-routing/SKILL.md").exists());
    assert!(!root.join(".agents").exists());
    assert!(!root.join(".claude").exists());

    let second = sync::sync_repo(root).expect("second sync");
    assert!(second.written_files.is_empty());
}

#[test]
fn cursor_fresh_rules_include_mdc_frontmatter_and_are_idempotent() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();

    sync::init_repo_with_adapter(root, Some(AdapterTarget::Cursor)).expect("init cursor");
    let mdc = fs::read_to_string(root.join(".cursor/rules/anvil.mdc")).expect("read mdc");
    assert!(mdc.starts_with("---\ndescription: Anvil deterministic harness discipline\nglobs:\nalwaysApply: true\n---\n\n<!-- anvil:managed:start -->"));

    let second = sync::sync_repo(root).expect("second sync");
    assert!(second.written_files.is_empty());
}

#[test]
fn windsurf_adapter_syncs_mcp_and_rules_idempotently() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    fs::create_dir_all(root.join(".windsurf/rules")).expect("mkdir windsurf rules");
    fs::write(
        root.join(".windsurf/mcp_config.json"),
        r#"{"mcpServers":{"github":{"command":"gh-mcp","args":[]}}}"#,
    )
    .expect("seed windsurf mcp");
    fs::write(
        root.join(".windsurf/rules/anvil.md"),
        "# Team Rules\n<!-- anvil:managed:start -->\nstale\n<!-- anvil:managed:end -->\n# Keep\n",
    )
    .expect("seed windsurf rules");

    sync::init_repo_with_adapter(root, Some(AdapterTarget::Windsurf)).expect("init windsurf");

    let mcp: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".windsurf/mcp_config.json")).expect("read windsurf mcp"),
    )
    .expect("parse windsurf mcp");
    assert_eq!(mcp["mcpServers"]["github"]["command"], "gh-mcp");
    assert_eq!(mcp["mcpServers"]["anvil"]["command"], "anvil");

    let md = fs::read_to_string(root.join(".windsurf/rules/anvil.md")).expect("read windsurf md");
    assert!(md.starts_with("# Team Rules\n<!-- anvil:managed:start -->"));
    assert!(!md.contains("\nstale\n"));
    assert!(md.ends_with("<!-- anvil:managed:end -->\n# Keep\n"));
    assert!(!root.join(".agents").exists());
    assert!(!root.join(".claude").exists());

    let second = sync::sync_repo(root).expect("second sync");
    assert!(second.written_files.is_empty());
}

#[test]
fn codex_adapter_merges_config_toml_and_agents_md_idempotently() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    fs::create_dir_all(root.join(".codex")).expect("mkdir codex");
    fs::write(
        root.join(".codex/config.toml"),
        "model = \"o3\"\n\n[mcp_servers.github]\ncommand = \"gh-mcp\"\n",
    )
    .expect("seed codex config");

    sync::init_repo_with_adapter(root, Some(AdapterTarget::Codex)).expect("init codex");

    let toml = fs::read_to_string(root.join(".codex/config.toml")).expect("read config.toml");
    assert!(toml.contains("model = \"o3\""));
    assert!(toml.contains("[mcp_servers.github]\ncommand = \"gh-mcp\""));
    assert!(toml.contains("[mcp_servers.anvil]\ncommand = \"anvil\"\nargs = [\"mcp\"]\n"));
    assert!(root.join("AGENTS.md").exists());
    assert!(!root.join(".agents").exists());
    assert!(!root.join(".claude").exists());

    let second = sync::sync_repo(root).expect("second sync");
    assert!(second.written_files.is_empty());
}

#[test]
fn codex_adapter_updates_existing_anvil_section_without_clobbering_neighbors() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    fs::create_dir_all(root.join(".codex")).expect("mkdir codex");
    fs::write(
        root.join(".codex/config.toml"),
        "[mcp_servers.anvil]\ncommand = \"old\"\nstartup_timeout_sec = 20\n\n[projects.\"/repo\"]\ntrust_level = \"trusted\"\n",
    )
    .expect("seed codex config");

    sync::init_repo_with_adapter(root, Some(AdapterTarget::Codex)).expect("init codex");

    let toml = fs::read_to_string(root.join(".codex/config.toml")).expect("read config.toml");
    assert!(toml.contains("command = \"anvil\""));
    assert!(toml.contains("args = [\"mcp\"]"));
    assert!(toml.contains("startup_timeout_sec = 20"));
    assert!(toml.contains("[projects.\"/repo\"]\ntrust_level = \"trusted\""));
    assert!(!toml.contains("command = \"old\""));

    let second = sync::sync_repo(root).expect("second sync");
    assert!(second.written_files.is_empty());
}

#[test]
fn all_adapter_auto_syncs_existing_cursor_without_polluting_absent_repos() {
    let empty_dir = tempdir().expect("empty tempdir");
    sync::init_repo_with_adapter(empty_dir.path(), Some(AdapterTarget::All)).expect("init all");
    assert!(!empty_dir.path().join(".cursor").exists());
    assert!(!empty_dir.path().join(".windsurf").exists());
    assert!(!empty_dir.path().join(".codex").exists());

    let cursor_dir = tempdir().expect("cursor tempdir");
    fs::create_dir_all(cursor_dir.path().join(".cursor")).expect("mkdir .cursor");
    sync::init_repo_with_adapter(cursor_dir.path(), Some(AdapterTarget::All))
        .expect("init all with .cursor");
    assert!(cursor_dir.path().join(".cursor/mcp.json").exists());
    assert!(cursor_dir.path().join(".cursor/rules/anvil.mdc").exists());
    assert!(cursor_dir.path().join(".cursor/skills/subagent-routing/SKILL.md").exists());
    assert!(cursor_dir.path().join(".agents/hooks.json").exists());
    assert!(cursor_dir.path().join(".claude/settings.json").exists());
    assert!(!cursor_dir.path().join(".windsurf").exists());
    assert!(!cursor_dir.path().join(".codex").exists());
}

#[test]
fn infer_default_adapter_detects_single_secondary_ide_and_defaults_empty_to_all() {
    let cursor_repo = tempdir().expect("cursor repo");
    fs::create_dir_all(cursor_repo.path().join(".cursor")).expect("mkdir .cursor");
    sync::init_repo(cursor_repo.path()).expect("init cursor repo");
    assert_eq!(
        AnvilConfig::load(cursor_repo.path()).expect("load").adapters,
        AdapterTarget::Cursor
    );

    let windsurf_repo = tempdir().expect("windsurf repo");
    fs::create_dir_all(windsurf_repo.path().join(".windsurf")).expect("mkdir .windsurf");
    sync::init_repo(windsurf_repo.path()).expect("init windsurf repo");
    assert_eq!(
        AnvilConfig::load(windsurf_repo.path()).expect("load").adapters,
        AdapterTarget::Windsurf
    );

    let codex_repo = tempdir().expect("codex repo");
    fs::create_dir_all(codex_repo.path().join(".codex")).expect("mkdir .codex");
    sync::init_repo(codex_repo.path()).expect("init codex repo");
    assert_eq!(AnvilConfig::load(codex_repo.path()).expect("load").adapters, AdapterTarget::Codex);

    let empty_repo = tempdir().expect("empty repo");
    sync::init_repo(empty_repo.path()).expect("init empty repo");
    assert_eq!(AnvilConfig::load(empty_repo.path()).expect("load").adapters, AdapterTarget::All);
}

#[test]
fn umbrella_root_docs_include_availability_guard() {
    let ws = tempdir().expect("tempdir");
    let sub_abs = ws.path().join("services/api");
    fs::create_dir_all(&sub_abs).expect("mkdir subrepo");
    let subrepos = [sync::UmbrellaSubrepo {
        rel_path: "services/api".to_owned(),
        abs_path: sub_abs,
        stacks: Vec::new(),
    }];

    sync::init_umbrella_root(ws.path(), Some(AdapterTarget::All), &subrepos)
        .expect("init_umbrella_root");

    for doc_name in ["AGENTS.md", "CLAUDE.md"] {
        let content = fs::read_to_string(ws.path().join(doc_name)).expect(doc_name);
        assert!(content.contains("Availability Guard"), "{doc_name} missing Availability Guard");
        assert!(content.contains("skip this entire block"), "{doc_name} missing skip instruction");
    }
}

#[test]
fn ide_specific_templates_multiline_codex_and_claude_hook_merge_preserve_neighbors() {
    let cursor_repo = tempdir().expect("cursor");
    sync::init_repo_with_adapter(cursor_repo.path(), Some(AdapterTarget::Cursor))
        .expect("cursor init");
    let mdc = fs::read_to_string(cursor_repo.path().join(".cursor/rules/anvil.mdc")).expect("mdc");
    assert!(mdc.contains("Cursor Thin Adapter"));
    assert!(mdc.contains(".cursor/mcp.json"));
    assert!(!mdc.contains("Antigravity Thin Adapter"));

    let codex_repo = tempdir().expect("codex");
    fs::create_dir_all(codex_repo.path().join(".codex")).expect("mkdir .codex");
    fs::write(
        codex_repo.path().join(".codex/config.toml"),
        "[mcp_servers.anvil]\ncommand = \"old\"\nargs = [\n  \"--old\",\n  \"mcp\",\n]\n\n[other]\nk = 1\n",
    )
    .expect("seed multiline toml");
    sync::init_repo_with_adapter(codex_repo.path(), Some(AdapterTarget::Codex))
        .expect("codex init");
    let toml = fs::read_to_string(codex_repo.path().join(".codex/config.toml")).expect("toml");
    assert!(toml.contains("args = [\"mcp\"]"));
    assert!(!toml.contains("\"--old\""));
    assert!(toml.contains("[other]\nk = 1"));
    let codex_agents =
        fs::read_to_string(codex_repo.path().join("AGENTS.md")).expect("codex agents");
    assert!(codex_agents.contains("Codex Thin Adapter"));
    assert!(!codex_agents.contains(".agents/mcp_config.json"));

    fs::remove_dir_all(codex_repo.path().join(".harness")).expect("clear .harness");
    sync::init_repo(codex_repo.path()).expect("re-infer codex with AGENTS.md");
    assert_eq!(AnvilConfig::load(codex_repo.path()).expect("cfg").adapters, AdapterTarget::Codex);

    let claude_repo = tempdir().expect("claude");
    fs::create_dir_all(claude_repo.path().join(".claude")).expect("mkdir .claude");
    fs::write(
        claude_repo.path().join(".claude/settings.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"*","hooks":[{"type":"command","command":"my-user-hook"},{"type":"command","command":"anvil hook old"}]}]}}"#,
    )
    .expect("seed co-located claude hooks");
    sync::init_repo_with_adapter(claude_repo.path(), Some(AdapterTarget::Claude))
        .expect("claude init");
    let settings: Value = serde_json::from_str(
        &fs::read_to_string(claude_repo.path().join(".claude/settings.json")).expect("read"),
    )
    .expect("parse");
    let hooks = settings["hooks"]["PreToolUse"][0]["hooks"].as_array().expect("hooks arr");
    assert_eq!(hooks.len(), 2);
    assert_eq!(hooks[0]["command"], "my-user-hook");
    assert_eq!(hooks[1]["command"], "anvil hook PreToolUse --adapter claude");
}

#[test]
fn sync_mirrors_guard_decisions_into_permissions_without_touching_user_rules() {
    use anvil_core::config::security_guard::{GuardCategory, GuardDecision};

    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    seed_existing_repo_fixtures(root);
    fs::write(
        root.join(".claude/settings.json"),
        r#"{"model":"sonnet","permissions":{"allow":["Bash(npm test)"],"deny":["Bash(curl:*)"],"ask":["Bash(git push:*)"]}}"#,
    )
    .expect("seed settings");
    sync::init_repo(root).expect("init_repo");

    let read_settings = || -> Value {
        serde_json::from_str(&fs::read_to_string(root.join(".claude/settings.json")).unwrap())
            .unwrap()
    };
    let list = |settings: &Value, key: &str| -> Vec<String> {
        settings["permissions"][key]
            .as_array()
            .map(|rules| rules.iter().filter_map(|r| r.as_str().map(str::to_owned)).collect())
            .unwrap_or_default()
    };

    let settings = read_settings();
    assert_eq!(settings["model"], "sonnet");
    assert_eq!(list(&settings, "allow"), ["Bash(npm test)"]);
    let deny = list(&settings, "deny");
    assert_eq!(deny[0], "Bash(curl:*)");
    for rule in [
        "Bash(aws ec2 terminate-instances:*)",
        "Bash(aws s3 rb:*)",
        "Bash(terraform destroy:*)",
        "Bash(kubectl delete:*)",
    ] {
        assert!(deny.iter().any(|r| r == rule), "default deny list lacks {rule}");
    }
    let ask = list(&settings, "ask");
    assert_eq!(ask[0], "Bash(git push:*)");
    assert!(ask.iter().any(|r| r == "Bash(docker system prune:*)"));
    assert!(sync::sync_repo(root).expect("second sync").written_files.is_empty());

    let mut config = AnvilConfig::load(root).expect("load config");
    config.security_guard.categories.set(GuardCategory::Cloud, GuardDecision::Ask);
    config.security_guard.categories.set(GuardCategory::Infra, GuardDecision::Allow);
    config.save(root).expect("save config");
    sync::sync_repo(root).expect("sync after config change");

    let settings = read_settings();
    let deny = list(&settings, "deny");
    assert_eq!(deny, ["Bash(curl:*)"]);
    let ask = list(&settings, "ask");
    assert!(ask.iter().any(|r| r == "Bash(aws s3 rb:*)"));
    assert!(!ask.iter().any(|r| r.contains("terraform")));
    assert_eq!(list(&settings, "allow"), ["Bash(npm test)"]);
}
