//! `anvil grep`: gitignore-aware, token-bounded code search annotated with enclosing symbols.

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_core::config::search::SearchBackend;
use anvil_core::diff_grep::{GrepOptions, GrepReport, grep_with_backend, render};
use anvil_graph::query::annotate_grep_report;
use anvil_graph::store;
use anyhow::Result;

use crate::cli::GrepArgs;
use crate::commands::ExitCodeError;

/// Exit status when the search itself fails (invalid regex, unknown path, bad backend).
const EXIT_ERROR: i32 = 2;
/// Exit status when the search succeeds without any match (ripgrep convention).
const EXIT_NO_MATCH: i32 = 1;

/// Executes `anvil grep`: exit 0 with matches, 1 without, 2 on error.
pub fn execute(root: &Path, args: &GrepArgs) -> Result<()> {
    let opts = to_options(args);
    let mut report = search(root, args, &opts)
        .map_err(|err| ExitCodeError::with_message(EXIT_ERROR, format!("{err:#}")))?;
    annotate_with_cached_graph(root, &mut report);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}", render(&report, opts.max_tokens));
    }
    if report.total_matches == 0 {
        return Err(ExitCodeError::silent(EXIT_NO_MATCH).into());
    }
    Ok(())
}

fn search(root: &Path, args: &GrepArgs, opts: &GrepOptions) -> Result<GrepReport> {
    let backend = match &args.backend {
        Some(raw) => SearchBackend::parse(raw)?,
        None => AnvilConfig::load_or_default(root)?.search.backend,
    };
    grep_with_backend(root, opts, backend)
}

fn to_options(args: &GrepArgs) -> GrepOptions {
    let mut opts = GrepOptions::new(&args.pattern);
    opts.paths = args.paths.clone();
    opts.globs = args.globs.clone();
    opts.ignore_case = args.ignore_case;
    opts.smart_case = !args.case_sensitive;
    opts.fixed_strings = args.fixed_strings;
    opts.word = args.word;
    opts.hidden = args.hidden;
    opts.context = args.context;
    opts.max_matches = args.max_matches;
    opts.max_per_file = args.max_per_file;
    opts.max_tokens = args.max_tokens;
    opts.files_only = args.files_only;
    opts
}

/// Adds `[fn name]` annotations when `anvil graph build` has produced a cached graph; never fails.
fn annotate_with_cached_graph(root: &Path, report: &mut GrepReport) {
    let bin_path = store::default_bin_path(root);
    if !bin_path.is_file() {
        return;
    }
    if let Ok(graph) = store::load_binary(&bin_path) {
        annotate_grep_report(&graph, report);
    }
}
