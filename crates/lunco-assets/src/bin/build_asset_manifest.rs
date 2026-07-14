//! Writes the engine-library `manifest.json` the **web build** ships.
//!
//! ```text
//! cargo run -p lunco-assets --bin build_asset_manifest -- <dist>/assets
//! ```
//!
//! The browser has no `readdir`, so the bundle has to carry its own table of
//! contents. This produces it — from the staged tree itself, at packaging time, so
//! the listing describes the bundle that actually shipped rather than one someone
//! compiled against.
//!
//! # Why this is a Rust binary and not four lines of shell
//!
//! Because "which files ship" already has a definition —
//! [`lunco_assets::discovery::scan_library`] — and the runtime reads the result. A
//! packaging step that re-derives it with its own `find`/`os.walk` is a *second*
//! implementation of the same rule, in another language, kept in step by discipline.
//! That is how the native and web builds come to disagree about what an asset is,
//! and it is the same defect as the `build.rs` bake this whole manifest replaced.
//!
//! The shell version this replaced was not a hypothetical drift. It walked hidden
//! directories, which `scan_library` skips — and it runs over the **staged** tree,
//! into which `build_web.sh` copies Twins, and a Twin carries `.lunco/runtime/*.usda`.
//! The web catalog would have listed a Twin's private runtime layers that native
//! discovery never sees.
//!
//! So the packager calls the scanner. One rule, one place.
//!
//! Native-only (it writes a file, and the web build runs it on the host).
#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;

fn main() {
    let Some(dir) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!(
            "usage: build_asset_manifest <assets-dir>\n\
             \n\
             Writes <assets-dir>/manifest.json — the file listing the web build's\n\
             asset catalogs read at boot."
        );
        std::process::exit(2);
    };

    if !dir.is_dir() {
        eprintln!("build_asset_manifest: {} is not a directory", dir.display());
        std::process::exit(1);
    }

    let rels = lunco_assets::discovery::scan_library(&dir);
    if rels.is_empty() {
        // Not fatal on its own, but it means the web spawn palette and shader
        // catalog will be empty — almost certainly a staging bug, so say so.
        eprintln!(
            "build_asset_manifest: WARNING — no assets found under {}. \
             The web catalogs will be empty.",
            dir.display()
        );
    }

    let json = match serde_json::to_string(&rels) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("build_asset_manifest: could not serialise manifest: {e}");
            std::process::exit(1);
        }
    };

    let out = dir.join("manifest.json");
    if let Err(e) = std::fs::write(&out, json) {
        eprintln!("build_asset_manifest: could not write {}: {e}", out.display());
        std::process::exit(1);
    }

    println!("{} asset(s) → {}", rels.len(), out.display());
}
