//! Stamps a wire-protocol fingerprint into every binary that links this crate.
//!
//! The browser build runs Modelica off the main thread in a `lunica_worker`
//! wasm that talks to the main app over `postMessage` using bincode-encoded
//! `WireMessage`/`WireResult` envelopes (see `src/worker_transport.rs`). Both
//! the main app and the worker compile THIS crate, so both bake in the same
//! `LUNCO_WIRE_BUILD_ID` — *when built from the same source*. If the shipped
//! worker wasm is stale (build_web's mtime gate misfired, a dependency like
//! rumoca moved and only the main bundle rebuilt, or a dist was hand-assembled
//! from two builds), the worker's baked id differs and the boot handshake in
//! `worker_transport` reports it loudly instead of letting every bincode
//! message silently mis-decode (`UUID parsing failed`, `unexpected end of
//! file`, MSL "33 docs" instead of 2670).
//!
//! The id is a hash of the workspace `Cargo.lock` (captures dep-version moves
//! that change wire types) plus this crate's `src/` tree (captures edits to the
//! `WireMessage`/`WireResult`/`ModelicaCommand` definitions). Any change bumps
//! the id, so a mismatched pair can never be mistaken for compatible.

use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::path::Path;

fn hash_file(path: &Path, hasher: &mut DefaultHasher) {
    if let Ok(bytes) = std::fs::read(path) {
        hasher.write(&bytes);
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn hash_dir(dir: &Path, hasher: &mut DefaultHasher) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = read.flatten().map(|e| e.path()).collect();
    // Deterministic order — `read_dir` yields entries in filesystem order.
    entries.sort();
    for path in entries {
        if path.is_dir() {
            hash_dir(&path, hasher);
        } else {
            hash_file(&path, hasher);
        }
    }
}

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let mut hasher = DefaultHasher::new();

    // Workspace lockfile: a rumoca (or any dep) bump changes the resolved rev
    // here, which is exactly the class of skew the source-mtime scan misses.
    for lock in [
        format!("{manifest}/../../Cargo.lock"),
        format!("{manifest}/Cargo.lock"),
    ] {
        hash_file(Path::new(&lock), &mut hasher);
    }

    // This crate's sources: the wire-type definitions live here. `rerun-if-
    // changed` on the dir itself catches files added/removed (not just edited).
    let src = format!("{manifest}/src");
    println!("cargo:rerun-if-changed={src}");
    hash_dir(Path::new(&src), &mut hasher);

    println!("cargo:rustc-env=LUNCO_WIRE_BUILD_ID={:016x}", hasher.finish());
}
