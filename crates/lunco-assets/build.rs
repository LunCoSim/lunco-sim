use std::path::PathBuf;

fn main() {
    // `include_dir!` embeds the complete authored asset tree, including the
    // namespaced Rhai tool libraries. Cargo tracks Rust source dependencies
    // automatically, but a newly added asset file is otherwise invisible to
    // its change detector and can leave a production binary with an older
    // embedded directory listing. Track the one tree this crate embeds so
    // additions, removals, and edits rebuild the authoritative asset snapshot.
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo supplies CARGO_MANIFEST_DIR"),
    );
    let asset_dir = manifest_dir.join("../../assets");
    println!("cargo:rerun-if-changed={}", asset_dir.display());
}
