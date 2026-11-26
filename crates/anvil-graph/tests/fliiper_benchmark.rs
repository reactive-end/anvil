//! Graph benchmark over a generated TypeScript workspace, or over `ANVIL_BENCH_REPO` when set.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anvil_core::config::AnvilConfig;
use anvil_graph::{CodeGraphEngine, builder, store};
use tempfile::TempDir;

const GENERATED_MODULES: usize = 240;

#[test]
fn graph_build_save_update_and_blast_radius_benchmark() {
    let generated = TempDir::new().expect("workspace tempdir");
    let repo_root = match std::env::var_os("ANVIL_BENCH_REPO") {
        Some(path) => PathBuf::from(path),
        None => {
            write_workspace(generated.path());
            generated.path().to_path_buf()
        }
    };
    let config = AnvilConfig::default();

    let cold_start = Instant::now();
    let mut engine = CodeGraphEngine::build(&repo_root, config.clone()).expect("cold build");
    let cold_ms = elapsed_ms(cold_start);
    let node_count = engine.graph.nodes.len();
    let file_count = engine.graph.file_hashes.len();
    assert!(node_count > 100, "expected substantial nodes, got {node_count}");

    let (save_ms, load_ms) = measure_store(&engine);
    let sample = pick_sample_file(&engine);
    engine.graph.file_hashes.insert(sample.clone(), "forced-stale-hash".to_owned());
    let inc_start = Instant::now();
    let updated = builder::update_file(&mut engine.graph, &repo_root, &sample, &config)
        .expect("single-file incremental update");
    let inc_ms = elapsed_ms(inc_start);
    assert!(updated);

    let bfs_start = Instant::now();
    let blast = engine.blast_radius(std::slice::from_ref(&sample), 2);
    let bfs_ms = elapsed_ms(bfs_start);
    if std::env::var_os("ANVIL_BENCH_REPO").is_none() {
        assert_eq!(file_count, GENERATED_MODULES);
        assert!(
            blast.impacted_files.iter().any(|item| item.file.ends_with("module_1.ts")),
            "module_1 imports module_0 and must be impacted: {:?}",
            blast.impacted_files
        );
    }

    println!(
        "BENCHMARK graph | files={file_count} nodes={node_count} edges={} | \
         cold_build={cold_ms:.2}ms save_bin={save_ms:.2}ms load_bin={load_ms:.2}ms \
         inc_single_file={inc_ms:.2}ms reverse_bfs={bfs_ms:.2}ms impacted={}",
        engine.graph.edges.len(),
        blast.impacted_files.len()
    );
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn measure_store(engine: &CodeGraphEngine) -> (f64, f64) {
    let tmp = TempDir::new().expect("store tempdir");
    let bin_path = tmp.path().join("graph.bin");
    let save_start = Instant::now();
    store::save_binary(&engine.graph, &bin_path).expect("save binary");
    let save_ms = elapsed_ms(save_start);
    let load_start = Instant::now();
    let loaded = store::load_binary(&bin_path).expect("load binary");
    assert_eq!(loaded.nodes.len(), engine.graph.nodes.len());
    (save_ms, elapsed_ms(load_start))
}

fn pick_sample_file(engine: &CodeGraphEngine) -> String {
    engine
        .graph
        .file_hashes
        .keys()
        .find(|k| k.ends_with("module_0.ts"))
        .or_else(|| engine.graph.file_hashes.keys().next())
        .cloned()
        .expect("at least one source file")
}

/// Writes a chain of modules where each one imports and calls its predecessor.
fn write_workspace(root: &Path) {
    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src");
    for idx in 0..GENERATED_MODULES {
        let mut body = String::new();
        if let Some(prev) = idx.checked_sub(1) {
            let _ = writeln!(body, "import {{ step{prev} }} from './module_{prev}';");
            let _ = writeln!(
                body,
                "export function step{idx}(x: number): number {{\n  return step{prev}(x) + 1;\n}}"
            );
        } else {
            let _ = writeln!(body, "export function step0(x: number): number {{\n  return x;\n}}");
        }
        let _ = writeln!(
            body,
            "export class Service{idx} {{\n  run(): number {{\n    return step{idx}({idx});\n  }}\n}}"
        );
        fs::write(src.join(format!("module_{idx}.ts")), body).expect("write module");
    }
}
