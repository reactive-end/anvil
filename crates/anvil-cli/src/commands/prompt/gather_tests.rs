//! End-to-end checks of what a prompt finds in a small graph: inflected words reach the symbols
//! named with their root, and the earlier quality cases still pick what they should.

use std::time::Duration;

use anvil_core::config::AnvilConfig;
use anvil_graph::{builder, store};

use super::gather::{Limits, gather};
use super::terms::extract;

/// Builds the graph of `files` under a temp dir, caches it where `gather` looks, and returns the
/// names of the code it finds for `prompt`.
fn found(prompt: &str, files: &[(&str, &str)]) -> Vec<String> {
    let dir = tempfile::tempdir().expect("temp dir");
    for (rel, body) in files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
        std::fs::write(path, body).expect("write file");
    }
    let config = AnvilConfig::default();
    let graph = builder::build_full(dir.path(), &config).expect("graph builds");
    let bin = store::default_bin_path(dir.path());
    std::fs::create_dir_all(bin.parent().expect("parent")).expect("create state dir");
    store::save_binary(&graph, &bin).expect("graph saved");
    let limits =
        Limits { code: 6, docs: 0, memories: 0, time: Duration::from_secs(10), grep: false };
    let out = gather(dir.path(), &config, &extract(prompt), limits);
    out.code.into_iter().map(|c| c.name).collect()
}

fn has(names: &[String], wanted: &str) -> bool {
    names.iter().any(|n| n == wanted)
}

const ORDER_FILES: [(&str, &str); 3] = [
    ("src/orders/cancel-order.use-case.ts", "export class CancelOrderUseCase {}\n"),
    ("src/orders/order.ts", "export class Order {}\n"),
    ("src/billing/invoice.ts", "export class Invoice {}\n"),
];

#[test]
fn an_inflected_prompt_word_finds_the_symbol_named_with_its_root() {
    let names = found("when an order is cancelled, release its stock", &ORDER_FILES);
    assert!(has(&names, "CancelOrderUseCase"), "{names:?}");
    assert!(!has(&names, "Invoice"), "{names:?}");
}

#[test]
fn a_spanish_inflection_finds_the_symbol_too() {
    let names = found("al ser cancelada la order, liberar el stock", &ORDER_FILES);
    assert!(has(&names, "CancelOrderUseCase"), "{names:?}");
}

#[test]
fn stale_concepts_and_search_repo_are_found_for_the_ranking_prompt() {
    let files = [
        ("src/docs/search.rs", "pub fn search_repo() {}\n"),
        ("src/check/docs.rs", "pub fn stale_concepts() {}\npub fn render_table() {}\n"),
        ("src/check/token_guard.rs", "pub fn check_read_token_guard() {}\n"),
    ];
    let names = found("make search_repo rank stale concepts lower", &files);
    assert!(has(&names, "search_repo") && has(&names, "stale_concepts"), "{names:?}");
    assert!(!has(&names, "render_table") && !has(&names, "check_read_token_guard"), "{names:?}");
}

#[test]
fn the_token_guard_prompt_finds_the_guard_and_not_unrelated_reads() {
    let files = [
        (
            "src/hook/token_guard.rs",
            "pub fn check_read_token_guard() {}\npub fn check_pdf_read() {}\n",
        ),
        ("src/hook/other.rs", "pub fn read_config() {}\npub fn huge_report() {}\n"),
    ];
    let names = found("make the token guard block huge pdf reads sooner", &files);
    assert!(has(&names, "check_read_token_guard"), "{names:?}");
    assert!(has(&names, "check_pdf_read"), "{names:?}");
    assert!(!has(&names, "read_config"), "{names:?}");
}

#[test]
fn the_discount_prompt_never_offers_a_seed_file() {
    let files = [
        ("orders-api/src/orders/domain/order.ts", "export class Order { quantity = 0 }\n"),
        ("orders-api/src/seed.ts", "export function seed() { const order = { quantity: 11 }; }\n"),
    ];
    let names = found("add a discount when quantity is over 10", &files);
    assert!(!names.iter().any(|n| n.contains("seed")), "{names:?}");
}
