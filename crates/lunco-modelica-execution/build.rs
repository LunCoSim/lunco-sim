//! Build identity for the browser's Modelica wire protocol.
//!
//! The worker and its page-side transport are separate wasm binaries. Hashing
//! the protocol owner and the serialized runtime contracts makes a stale pair
//! fail during the handshake instead of silently decoding incompatible bytes.

#![allow(clippy::disallowed_methods)]

mod build_identity {
    include!("../../scripts/build_identity.rs");
}

use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::path::Path;

fn hash_file(path: &Path, hasher: &mut DefaultHasher) {
    println!("cargo:rerun-if-changed={}", path.display());
    let bytes = std::fs::read(path)
        .unwrap_or_else(|error| panic!("cannot hash protocol source {}: {error}", path.display()));
    hasher.write(&bytes);
}

fn hash_dir(dir: &Path, hasher: &mut DefaultHasher) {
    let read = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("cannot read protocol source {}: {error}", dir.display()));
    let mut entries: Vec<_> = read
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()
        .unwrap_or_else(|error| {
            panic!(
                "cannot enumerate protocol source {}: {error}",
                dir.display()
            )
        });
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

    hash_file(
        Path::new(&format!("{manifest}/../../Cargo.lock")),
        &mut hasher,
    );
    hash_dir(Path::new(&format!("{manifest}/src")), &mut hasher);
    hash_dir(
        Path::new(&format!("{manifest}/../lunco-modelica-runtime/src")),
        &mut hasher,
    );
    hash_dir(
        Path::new(&format!("{manifest}/../lunco-experiments/src")),
        &mut hasher,
    );
    hash_file(
        Path::new(&format!(
            "{manifest}/../lunco-modelica-library/src/worker_bridge.rs"
        )),
        &mut hasher,
    );

    println!(
        "cargo:rustc-env=LUNCO_WIRE_BUILD_ID={:016x}",
        hasher.finish()
    );
    build_identity::stamp();
}
