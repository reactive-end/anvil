use anvil_docs::frontmatter::{Document, FrontmatterError};
use anvil_docs::meta::Stamp;
use anvil_docs::trust::{Trust, trust_of};

const TEXT: &str = "---\ntype: Code Module\nx-custom: {a: 1, b: [x, y]}\ntitle: T\nx-anvil-paths:\n  - src/**\nzeta: last\n---\n# Body\r\n\n  trailing spaces  \n<!-- keep -->";
const BODY: &str = "# Body\r\n\n  trailing spaces  \n<!-- keep -->";

fn keys(doc: &Document) -> Vec<String> {
    doc.meta().expect("meta").keys().filter_map(|k| k.as_str().map(str::to_owned)).collect()
}

#[test]
fn untouched_document_renders_byte_for_byte() {
    let doc = Document::parse(TEXT).expect("parse");
    assert_eq!(doc.render(), TEXT);
    assert_eq!(doc.body(), BODY);
}

#[test]
fn edits_keep_unknown_keys_order_and_body() {
    let mut doc = Document::parse(TEXT).expect("parse");
    doc.set_str("title", "Renamed");
    doc.set_generated(&Stamp { by: "anvil/0.4.0".into(), at: "2026-01-01T00:00:00Z".into() });
    let again = Document::parse(&doc.render()).expect("reparse");
    assert_eq!(keys(&again), ["type", "x-custom", "title", "x-anvil-paths", "zeta", "generated"]);
    assert_eq!(again.title(), Some("Renamed"));
    assert_eq!(again.body(), BODY);
    let custom = again.field("x-custom").expect("x-custom kept");
    assert_eq!(custom["a"].as_i64(), Some(1));
    assert_eq!(custom["b"][1].as_str(), Some("y"));
    assert_eq!(again.code_paths(), ["src/**"]);
}

#[test]
fn removing_a_key_keeps_the_order_of_the_rest() {
    let mut doc = Document::parse(TEXT).expect("parse");
    doc.remove_field("x-custom");
    assert_eq!(keys(&doc), ["type", "title", "x-anvil-paths", "zeta"]);
}

#[test]
fn files_without_frontmatter_are_all_body() {
    let doc = Document::parse("# Just a readme\n").expect("parse");
    assert!(doc.meta().is_none());
    assert_eq!(doc.render(), "# Just a readme\n");
}

#[test]
fn malformed_frontmatter_is_reported() {
    assert_eq!(Document::parse("---\ntype: x\n"), Err(FrontmatterError::Unterminated));
    assert_eq!(Document::parse("---\n- a\n- b\n---\n"), Err(FrontmatterError::NotMapping));
    assert!(matches!(Document::parse("---\na: [1\n---\n"), Err(FrontmatterError::Yaml(_))));
}

fn doc_with(generated: &str, verified: &[(&str, &str)]) -> Document {
    let mut text = format!("---\ntype: T\ngenerated: {{by: anvil/0.4.0, at: \"{generated}\"}}\n");
    if !verified.is_empty() {
        text.push_str("verified:\n");
        for (by, at) in verified {
            text.push_str(&format!("  - {{by: \"{by}\", at: \"{at}\"}}\n"));
        }
    }
    text.push_str("---\n");
    Document::parse(&text).expect("parse")
}

#[test]
fn trust_tiers() {
    let none = doc_with("2026-02-01T00:00:00Z", &[]);
    assert_eq!(trust_of(&none), Trust::Unverified);
    let machine = doc_with("2026-02-01T00:00:00Z", &[("ci/1.0", "2026-01-01T00:00:00Z")]);
    assert_eq!(trust_of(&machine), Trust::MachineConfirmed);
    let human = doc_with("2026-02-01T00:00:00Z", &[("human:Ada", "2026-02-01T00:00:00Z")]);
    assert_eq!(trust_of(&human), Trust::HumanReviewed);
    let later = doc_with("2026-02-01T00:00:00Z", &[("human:Ada", "2026-03-01T00:00:00Z")]);
    assert_eq!(trust_of(&later), Trust::HumanReviewed);
}

#[test]
fn human_verification_older_than_generation_does_not_count() {
    let old = doc_with("2026-02-01T00:00:00Z", &[("human:Ada", "2026-01-31T23:59:59Z")]);
    assert_eq!(trust_of(&old), Trust::Unverified);
    let mixed = doc_with(
        "2026-02-01T00:00:00Z",
        &[("human:Ada", "2026-01-01T00:00:00Z"), ("ci/1.0", "2026-02-02T00:00:00Z")],
    );
    assert_eq!(trust_of(&mixed), Trust::MachineConfirmed);
}

#[test]
fn awkward_strings_survive_serialisation() {
    let awkward = [
        "Orders: placement and lifecycle",
        "uses # hash and 'quotes' and \"double quotes\"",
        "- starts like a list item",
        "yes",
        "0.3",
        "2026-11-27T10:00:00Z",
        "null",
        "line one\nline two",
        "caf\u{e9} \u{2014} unicode",
        "{braces} [brackets] & *star !bang %pct @at `tick`",
        "",
    ];
    for text in awkward {
        let mut doc = Document::parse("---\ntype: T\n---\nbody\n").expect("parse");
        doc.set_str("title", text);
        let again = Document::parse(&doc.render()).expect("reparse");
        assert_eq!(again.title(), Some(text), "rendered as:\n{}", doc.render());
        assert_eq!(again.body(), "body\n");
    }
}
