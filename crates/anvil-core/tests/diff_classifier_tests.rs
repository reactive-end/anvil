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
fn tsx_component_logic_change_requires_ui_tier2() {
    let hunk = "@@ -10,3 +10,3 @@\n-  <button onClick={save}>Save</button>\n+  <button onClick={submit} disabled={busy}>Submit</button>\n";
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

    let deps = classify_paths(&["package.json", "src/utils.ts"]);
    assert_eq!(deps.primary_category, DiffCategory::Deps);
    assert_eq!(deps.minimum_tier, Tier::Tier2);
    assert!(deps.needs_deps_audit);

    let contract = classify_paths(&["proto/orders.proto", "src/service.ts"]);
    assert_eq!(contract.primary_category, DiffCategory::Contract);
    assert_eq!(contract.minimum_tier, Tier::Tier1);
}
