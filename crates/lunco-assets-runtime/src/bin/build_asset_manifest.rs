//! Writes the engine-library `manifest.json` the **web build** ships.
//!
//! ```text
//! cargo run -p lunco-assets-runtime --bin build_asset_manifest -- <dist>/assets
//! ```
//!
//! The browser has no `readdir`, so the bundle has to carry its own table of
//! contents. This produces it — from the staged tree itself, at packaging time, so
//! the listing describes the bundle that actually shipped rather than one someone
//! compiled against.
//!
//! [`lunco_assets_runtime::discovery::scan_library`] owns catalog selection,
//! containment and diagnostics. Inventory failure exits before writing a manifest.
//!
//! Native-only (it writes a file, and the web build runs it on the host).
#![cfg_attr(not(target_arch = "wasm32"), allow(clippy::disallowed_methods))]

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
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

    let rels = match lunco_assets_runtime::discovery::scan_library(&dir) {
        Ok(rels) => rels,
        Err(error) => {
            eprintln!(
                "build_asset_manifest: cannot inventory {}: {error}",
                dir.display()
            );
            std::process::exit(1);
        }
    };
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

    let out = lunco_assets_core::asset_manifest_path(&dir);
    if let Err(e) = std::fs::write(&out, json) {
        eprintln!(
            "build_asset_manifest: could not write {}: {e}",
            out.display()
        );
        std::process::exit(1);
    }

    println!("{} asset(s) → {}", rels.len(), out.display());
}

#[cfg(target_arch = "wasm32")]
fn main() {}
