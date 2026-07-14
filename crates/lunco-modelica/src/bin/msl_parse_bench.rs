//! Throwaway benchmark: compare the two MSL parse paths head-to-head.
//!
//! - `perfile` → `indexer::parse_native_msl_bundle` (one file at a time —
//!   what `msl_indexer` and the workbench cold path now both use).
//! - `batch`   → rumoca's `parse_source_root_with_cache_in` (the old
//!   workbench cold path — a single all-files rayon batch).
//!
//! One mode per process so each cold run starts with an empty in-memory
//! cache. Clear `<cache>/rumoca` between cold runs from the shell. Pair
//! with `/usr/bin/time -v` to capture peak RSS (the metric that decides
//! whether a weak machine swaps).

// Native-only benchmark: it drives `indexer::parse_native_msl_bundle` (a
// `#[cfg(not(wasm32))]` module) over an on-disk MSL tree, which does not exist
// in a browser. The whole body lives in `mod native` so `wasm32` sees only the
// stub `main` below — nothing here is meant to run, or lint, on the web.
#[cfg(not(target_arch = "wasm32"))]
mod native {
use std::time::Instant;

fn walk_mo(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk_mo(&p, out);
            } else if p.extension().is_some_and(|x| x == "mo") {
                out.push(p);
            }
        }
    }
}

pub(crate) fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    // arg 2 = rumoca cache root (rumoca ignores RUMOCA_CACHE_DIR; the cache
    // root is a programmatic override). A fresh dir = true cold parse; reuse
    // it = warm. Lets us bench without touching the user's XDG cache.
    if let Some(cache) = std::env::args().nth(2) {
        rumoca_compile::source_roots::set_cache_root_override(std::path::PathBuf::from(cache));
    }

    // Reproduce the in-app condition: when a logger sets log's max level to
    // TRACE (Bevy's tracing-log bridge does exactly this), parol_runtime's
    // hot-path `log::trace!` calls construct + format records (Display-format
    // the whole token stream) even with no actual sink. LUNCO_LOG_MAX=trace
    // simulates it without a subscriber.
    if std::env::var("LUNCO_LOG_MAX").as_deref() == Ok("trace") {
        log::set_max_level(log::LevelFilter::Trace);
        eprintln!("[bench] log max level forced to TRACE");
    }

    let t = Instant::now();
    match mode.as_str() {
        "perfile" => {
            let docs = lunco_modelica::indexer::parse_native_msl_bundle();
            println!(
                "perfile: {} docs in {:.2}s",
                docs.len(),
                t.elapsed().as_secs_f64()
            );
        }
        "bundle" => {
            // Time loading the cached parsed-msl.bin (the warm fast path):
            // zstd + bincode decode of the prebuilt bundle, no parsing.
            match lunco_modelica::msl_remote::parsed_msl_bundle() {
                Some(b) => println!(
                    "bundle-decode: {} docs in {:.2}s",
                    b.len(),
                    t.elapsed().as_secs_f64()
                ),
                None => println!("bundle-decode: no parsed-msl.bin on disk"),
            }
        }
        "raw" => {
            // Exact production code path (raw parse_to_ast + dedicated pool).
            // Thread count via LUNCO_MSL_PARSE_THREADS. Standalone = clean
            // floor (no Bevy contention).
            let docs = lunco_modelica::indexer::parse_native_msl_bundle();
            println!(
                "raw(threads={}): {} docs in {:.2}s",
                std::env::var("LUNCO_MSL_PARSE_THREADS").unwrap_or_else(|_| "auto".into()),
                docs.len(),
                t.elapsed().as_secs_f64()
            );
        }
        "chunked" => {
            // Parse all native MSL files in fixed-size parallel chunks: up
            // to N files in flight at once (bounded peak), N-way parallel.
            let n: usize = std::env::args()
                .nth(3)
                .and_then(|s| s.parse().ok())
                .unwrap_or(8);
            let mut paths: Vec<std::path::PathBuf> = Vec::new();
            walk_mo(&lunco_assets::msl_dir(), &mut paths);
            // Retain the full bundle (the real output) so peak RSS is
            // comparable to perfile/batch, which both build it.
            let mut bundle: Vec<(String, rumoca_compile::parsing::ast::StoredDefinition)> =
                Vec::with_capacity(paths.len());
            for chunk in paths.chunks(n) {
                if let Ok(pairs) = rumoca_compile::parsing::parse_files_parallel(chunk) {
                    bundle.extend(pairs);
                }
            }
            println!(
                "chunked(N={n}): {} docs in {:.2}s",
                bundle.len(),
                t.elapsed().as_secs_f64()
            );
        }
        "batch" => {
            let root = lunco_assets::msl_source_root_path().expect("no MSL root on disk");
            let cache = rumoca_compile::source_roots::resolve_source_root_cache_dir();
            let parsed = rumoca_compile::source_roots::parse_source_root_with_cache_in(
                &root,
                cache.as_deref(),
            )
            .expect("parse_source_root_with_cache_in failed");
            println!(
                "batch:   {} docs in {:.2}s (cache {:?})",
                parsed.documents.len(),
                t.elapsed().as_secs_f64(),
                parsed.cache_status
            );
        }
        other => {
            eprintln!("usage: msl_parse_bench perfile|batch (got {other:?})");
            std::process::exit(2);
        }
    }
}
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    native::main();
}

#[cfg(target_arch = "wasm32")]
fn main() {}
