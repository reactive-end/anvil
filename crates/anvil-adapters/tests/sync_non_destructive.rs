use std::fs;
use std::path::Path;

use serde_json::Value;
use tempfile::tempdir;

use anvil_adapters::sync;
use anvil_adapters::templates;

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
    assert!(templates::AGENTS_MD.lines().count() < 120);
    assert!(templates::CLAUDE_MD.lines().count() < 120);
    assert!(templates::AGENTS_MD.contains("<!-- anvil:managed:start -->"));
    assert!(templates::CLAUDE_MD.contains("<!-- anvil:managed:start -->"));
    assert_eq!(templates::EMBEDDED_SKILLS.len(), 4);

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
        assert!(root.join(".harness/skills").join(skill.name).join("SKILL.md").exists());
        assert!(root.join(".agents/skills").join(skill.name).join("SKILL.md").exists());
        assert!(root.join(".claude/skills").join(skill.name).join("SKILL.md").exists());
    }
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
    let pre_hooks = claude_settings["hooks"]["PreToolUse"]
        .as_array()
        .expect("PreToolUse array");
    assert_eq!(pre_hooks.len(), 2);
    assert_eq!(pre_hooks[0]["hooks"][0]["command"], "custom-lint-guard");
    assert_eq!(
        pre_hooks[1]["hooks"][0]["command"],
        "anvil hook PreToolUse --adapter claude"
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

    let claude_mcp: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".mcp.json")).expect("read .mcp.json"),
    )
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
