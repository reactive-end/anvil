use anvil_core::diff_classifier::{ChangedFile, classify_files, classify_paths};
use anvil_core::traits::{DiffCategory, Tier};

#[test]
fn button_color_css_change_classifies_as_style_only_tier1_without_logic_tests() {
    let diff = classify_files(&[ChangedFile::with_hunks(
        "src/components/Button.css",
        "@@ -1,3 +1,3 @@\n-.btn-primary { background-color: #1d4ed8; }\n+.btn-primary { background-color: #2563eb; }\n",
    )]);

    assert_eq!(diff.primary_category, DiffCategory::StyleOnly);
    assert_eq!(diff.minimum_tier, Tier::Tier1);
    assert!(diff.skip_logic_tests);
    assert!(diff.needs_visual_qa);
    assert!(!diff.needs_deps_audit);
}

#[test]
fn tsx_classname_only_hunk_classifies_as_style_only() {
    let hunk = "@@ -10,3 +10,3 @@\n-  className=\"bg-blue-500\"\n+  className=\"bg-emerald-600\"\n";
    let diff = classify_files(&[ChangedFile::with_hunks("src/Button.tsx", hunk)]);
    assert_eq!(diff.primary_category, DiffCategory::StyleOnly);
    assert!(diff.skip_logic_tests);
}

#[test]
fn tsx_jsx_tag_classname_and_style_only_hunk_classifies_as_style_only() {
    let hunk = concat!(
        "@@ -10,4 +10,4 @@\n",
        "   <div>\n",
        "-    <button onClick={save} className=\"bg-blue-500\" style={{ color: 'red' }}>Save</button>\n",
        "+    <button onClick={save} className=\"bg-emerald-600\" style={{ color: 'blue' }}>Save</button>\n",
        "   </div>\n",
    );
    let diff = classify_files(&[ChangedFile::with_hunks("src/Button.tsx", hunk)]);
    assert_eq!(diff.primary_category, DiffCategory::StyleOnly);
    assert_eq!(diff.minimum_tier, Tier::Tier1);
    assert!(diff.skip_logic_tests);
    assert!(diff.needs_visual_qa);
}

#[test]
fn tsx_component_logic_change_requires_ui_tier2() {
    let hunk = "@@ -10,3 +10,3 @@\n-  <button onClick={save} className=\"a\">Save</button>\n+  <button onClick={submit} className=\"b\" disabled={busy}>Submit</button>\n";
    let diff = classify_files(&[ChangedFile::with_hunks("src/Button.tsx", hunk)]);
    assert_eq!(diff.primary_category, DiffCategory::Ui);
    assert_eq!(diff.minimum_tier, Tier::Tier2);
    assert!(!diff.skip_logic_tests);
    assert!(diff.needs_visual_qa);
}

#[test]
fn docs_and_deps_and_contracts_classify_accurately() {
    let docs = classify_paths(&["README.md", "docs/architecture.md"]);
    assert_eq!(docs.primary_category, DiffCategory::Docs);
    assert_eq!(docs.minimum_tier, Tier::Tier0);
    assert!(docs.skip_logic_tests);

    for manifest in [
        "package.json",
        "Cargo.toml",
        "requirements.txt",
        "build.gradle",
        "gradle/libs.versions.toml",
    ] {
        let deps = classify_paths(&[manifest]);
        assert_eq!(deps.primary_category, DiffCategory::Deps, "manifest={manifest}");
        assert_eq!(deps.minimum_tier, Tier::Tier2);
        assert!(deps.needs_deps_audit);
    }

    for contract_path in [
        "proto/orders.proto",
        "contracts/orders.ts",
        "services/api/contracts/user.ts",
    ] {
        let contract = classify_paths(&[contract_path, "src/service.ts"]);
        assert_eq!(contract.primary_category, DiffCategory::Contract, "path={contract_path}");
        assert_eq!(contract.minimum_tier, Tier::Tier1);
    }
}

#[test]
fn style_classifier_never_bypasses_onclick_or_background_sync_calls() {
    let onclick_bypass = "@@ -5,2 +5,2 @@\n- className=\"btn\"\n+ className=\"btn\" onClick={deleteDb}\n";
    let d1 = classify_files(&[ChangedFile::with_hunks("src/Danger.tsx", onclick_bypass)]);
    assert_eq!(d1.primary_category, DiffCategory::Ui);
    assert_eq!(d1.minimum_tier, Tier::Tier2);
    assert!(!d1.skip_logic_tests);

    let bg_sync = "@@ -5,2 +5,2 @@\n- className=\"btn\"\n+ backgroundSync()\n";
    let d2 = classify_files(&[ChangedFile::with_hunks("src/Danger.tsx", bg_sync)]);
    assert_eq!(d2.primary_category, DiffCategory::Ui);
    assert!(!d2.skip_logic_tests);

    let pure_css_prop = "@@ -5,2 +5,2 @@\n-  background-color: red;\n+  background-color: blue;\n";
    let d3 = classify_files(&[ChangedFile::with_hunks("src/Widget.html", pure_css_prop)]);
    assert_eq!(d3.primary_category, DiffCategory::StyleOnly);
    assert!(d3.skip_logic_tests);
}

#[test]
fn vue_svelte_and_astro_classify_as_ui_or_style_only() {
    for path in ["src/App.vue", "src/Page.svelte", "src/Layout.astro"] {
        let ui = classify_files(&[ChangedFile::with_hunks(
            path,
            "@@ -1,2 +1,2 @@\n-<button>Old</button>\n+<button on:click={run}>New</button>\n",
        )]);
        assert_eq!(ui.primary_category, DiffCategory::Ui, "path={path}");

        let style = classify_files(&[ChangedFile::with_hunks(
            path,
            "@@ -1,2 +1,2 @@\n-  color: #111;\n+  color: #222;\n",
        )]);
        assert_eq!(style.primary_category, DiffCategory::StyleOnly, "path={path}");
    }
}
