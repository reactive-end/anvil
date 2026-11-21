//! Read-only benchmark against `C:\Development\Fliiper\fliiper\fliiper-front` (if present).

use std::path::Path;
use std::time::Instant;

use anvil_core::config::AnvilConfig;
use anvil_graph::{CodeGraphEngine, builder, store};
use tempfile::TempDir;

#[test]
fn fliiper_front_readonly_benchmark() {
    let repo_root = Path::new(r"C:\Development\Fliiper\fliiper\fliiper-front");
    if !repo_root.is_dir() {
        eprintln!("Skipping fliiper-front benchmark: path not found");
        return;
    }

    let config = AnvilConfig::default();

    // 1. Cold build
    let cold_start = Instant::now();
    let mut engine = CodeGraphEngine::build(repo_root, config.clone()).expect("cold build");
    let cold_ms = cold_start.elapsed().as_secs_f64() * 1000.0;

    let node_count = engine.graph.nodes.len();
    let edge_count = engine.graph.edges.len();
    let file_count = engine.graph.file_hashes.len();
    assert!(node_count > 100, "expected substantial nodes in fliiper-front, got {node_count}");

    // 2. Binary save & load in a temporary directory (strictly read-only on fliiper-front)
    let tmp = TempDir::new().expect("tempdir");
    let bin_path = tmp.path().join("graph.bin");
    let save_start = Instant::now();
    store::save_binary(&engine.graph, &bin_path).expect("save binary");
    let save_ms = save_start.elapsed().as_secs_f64() * 1000.0;

    let load_start = Instant::now();
    let _loaded = store::load_binary(&bin_path).expect("load binary");
    let load_ms = load_start.elapsed().as_secs_f64() * 1000.0;

    // 3. Single-file incremental update (forced re-parse by invalidating 1 in-memory hash; zero disk writes)
    let sample_file = engine
        .graph
        .file_hashes
        .keys()
        .find(|k| k.ends_with(".component.ts"))
        .or_else(|| engine.graph.file_hashes.keys().next())
        .cloned()
        .expect("at least one source file");
    engine
        .graph
        .file_hashes
        .insert(sample_file.clone(), "forced-stale-hash".to_owned());

    let inc_start = Instant::now();
    let updated = builder::update_file(&mut engine.graph, repo_root, &sample_file, &config)
        .expect("single-file incremental update");
    let inc_ms = inc_start.elapsed().as_secs_f64() * 1000.0;
    assert!(updated);

    // 4. Reverse-BFS blast radius
    let bfs_start = Instant::now();
    let blast = engine.blast_radius(std::slice::from_ref(&sample_file), 2);
    let bfs_ms = bfs_start.elapsed().as_secs_f64() * 1000.0;

    println!(
        "BENCHMARK fliiper-front | files={file_count} nodes={node_count} edges={edge_count} | \
         cold_build={cold_ms:.2}ms save_bin={save_ms:.2}ms load_bin={load_ms:.2}ms \
         inc_single_file={inc_ms:.2}ms reverse_bfs={bfs_ms:.2}ms impacted={}",
        blast.impacted_files.len()
    );
}
