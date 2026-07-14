//! CLI wrapper for the MSL indexer.
//!
//! The actual indexer lives in `lunco_modelica::indexer` as a library
//! entry point so the workbench can drive the same workflow in-process
//! on `AsyncComputeTaskPool` after a fresh MSL download. This binary
//! is a thin shim: parse CLI args, hand off to `indexer::run`.
//!
//! Native-only: `lunco_modelica::indexer` is `#[cfg(not(wasm32))]` (it walks a
//! filesystem the browser doesn't have), and indexing is a build/host step —
//! the web consumes the artifacts it writes. The wasm stub exists only so the
//! bin target still has a `main` when the workspace is checked/linted for
//! `wasm32-unknown-unknown`.

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let opts = lunco_modelica::indexer::Options::parse();
    lunco_modelica::indexer::run(opts);
}

#[cfg(target_arch = "wasm32")]
fn main() {}
