//! Recall from a fresh agent's point of view: natural queries and onboarding context.

use anvil_memory::{MemoryEngine, MemoryKind, ObservationInput};
use tempfile::TempDir;

fn save(engine: &mut MemoryEngine, kind: MemoryKind, topic: &str, title: &str, what: &str) {
    let mut input = ObservationInput::new(kind, title, what, "why", "where", "learned");
    input.topic_key = Some(topic.to_owned());
    engine.save(input).expect("save");
}

fn seeded() -> (TempDir, MemoryEngine) {
    let tmp = TempDir::new().expect("tempdir");
    let mut engine = MemoryEngine::open(tmp.path(), "tester").expect("open");
    for n in 0..20 {
        save(
            &mut engine,
            MemoryKind::Decision,
            &format!("decision-{n}"),
            &format!("Architecture decision number {n}"),
            "Crates depend only on core contracts.",
        );
    }
    save(
        &mut engine,
        MemoryKind::Gotcha,
        "gotcha:install",
        "How to install the binary after a build",
        "Rename the running executable, then copy the new one.",
    );
    save(
        &mut engine,
        MemoryKind::Domain,
        "domain:rules",
        "User working rules: reply in Spanish, push only when asked",
        "Reply to the user in Spanish; commit or push only on request.",
    );
    save(
        &mut engine,
        MemoryKind::Domain,
        "domain:followups",
        "Open follow-ups: unfixed findings",
        "The daemon accept loop swallows errors and is not fixed yet.",
    );
    (tmp, engine)
}

#[test]
fn natural_queries_find_records_that_match_only_some_words() {
    let (_tmp, engine) = seeded();
    let top = |query: &str| {
        let hits = engine.search(query, None, 5).expect("search");
        hits.first().map(|hit| hit.topic_key.clone())
    };
    assert_eq!(top("what language should I talk to the user in").as_deref(), Some("domain:rules"));
    assert_eq!(top("can I push to the remote whenever I finish").as_deref(), Some("domain:rules"));
    assert_eq!(top("known unfixed bug in the daemon").as_deref(), Some("domain:followups"));
    assert_eq!(
        top("I just built a new release, how do I install it").as_deref(),
        Some("gotcha:install")
    );
}

#[test]
fn records_matching_every_word_rank_above_partial_matches() {
    let (_tmp, engine) = seeded();
    let hits = engine.search("install binary", None, 5).expect("search");
    assert_eq!(hits.first().map(|hit| hit.topic_key.as_str()), Some("gotcha:install"));
    assert!(engine.search("zzz-no-such-word", None, 5).expect("search").is_empty());
}

#[test]
fn onboarding_context_shows_rules_and_pitfalls_even_with_many_decisions() {
    let (_tmp, engine) = seeded();
    let context = engine.session_context(300).expect("context");
    let topics: Vec<&str> = context.iter().map(|hit| hit.topic_key.as_str()).collect();
    assert!(topics.contains(&"domain:rules"), "{topics:?}");
    assert!(topics.contains(&"domain:followups"), "{topics:?}");
    assert!(topics.contains(&"gotcha:install"), "{topics:?}");
    assert!(topics.iter().any(|topic| topic.starts_with("decision-")), "{topics:?}");
}
