//! Build-time MSL bundler for the web target.
//!
//! Reads the on-disk MSL tree (whatever `lunco_assets::msl_source_root_path`
//! points at on this host), packs every `.mo` source file into a tarball,
//! zstd-compresses it, hashes the result, and emits both the bundle and a
//! manifest into the chosen output directory.
//!
//! Output layout:
//!
//! ```text
//! <out>/
//!   manifest.json           # { msl_root_marker, sources_blob, sources_sha256, ... }
//!   sources-<sha8>.tar.zst  # tar of *.mo files relative to MSL root
//! ```
//!
//! The wasm runtime fetches `manifest.json` first, then the blob whose name
//! is in the manifest. Hashed filenames mean a deploy can never serve a stale
//! manifest against a freshened blob.
//!
//! ## Usage
//!
//! ```bash
//! cargo run -p lunco-assets --bin build_msl_assets -- \
//!     --out dist/modelica_workbench_web/msl
//! ```
//!
//! No CLI deps — uses bare `std::env::args` to keep this binary cheap to
//! compile.
//!
//! ## What gets packed
//!
//! - All `.mo` files under MSL root (recursive).
//! - Top-level `.mo` files like `Complex.mo`, `ObsoleteModelica4.mo`.
//! - Skipped: `Resources/` images and matrix data. Step 1b will add these
//!   once we know the wasm runtime needs them — most compile paths don't.

#![cfg(not(target_arch = "wasm32"))]

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufWriter, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut out_dir: Option<PathBuf> = None;
    let mut msl_root_override: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                i += 1;
                out_dir = Some(PathBuf::from(&args[i]));
            }
            "--msl-root" => {
                i += 1;
                msl_root_override = Some(PathBuf::from(&args[i]));
            }
            "-h" | "--help" => {
                eprintln!("usage: build_msl_assets --out <dir> [--msl-root <dir>]");
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let Some(out_dir) = out_dir else {
        eprintln!("error: --out is required");
        std::process::exit(2);
    };

    let msl_root = msl_root_override
        .or_else(lunco_assets::msl_source_root_path)
        .unwrap_or_else(|| {
            eprintln!(
                "error: no MSL tree on disk (run `lunco-assets -- download` first \
                 or pass --msl-root)"
            );
            std::process::exit(1);
        });
    eprintln!("MSL root: {}", msl_root.display());
    eprintln!("Output:   {}", out_dir.display());

    fs::create_dir_all(&out_dir).expect("create out dir");

    let mut entries: Vec<PathBuf> = Vec::new();
    collect_mo_files(&msl_root, &msl_root, &mut entries);
    entries.sort(); // deterministic tar order → reproducible hash
    eprintln!("found {} .mo files", entries.len());

    // Tar → zstd → write to a temp file, then rename to hashed final path.
    let tmp_path = out_dir.join("sources.tar.zst.tmp");
    let total_uncompressed = pack(&msl_root, &entries, &tmp_path);

    let sha = file_sha256(&tmp_path);
    let short = &sha[..16];
    let final_name = format!("sources-{short}.tar.zst");
    let final_path = out_dir.join(&final_name);
    fs::rename(&tmp_path, &final_path).expect("rename to final");
    let compressed_size = fs::metadata(&final_path).expect("stat final").len();

    let manifest = serde_json::json!({
        "schema_version": 1,
        "sources": {
            "filename": final_name,
            "sha256": sha,
            "uncompressed_bytes": total_uncompressed,
            "compressed_bytes": compressed_size,
            "file_count": entries.len(),
        },
        "msl_root_marker": "Modelica/package.mo",
    });
    let manifest_path = out_dir.join("manifest.json");
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("serialise manifest");
    fs::write(&manifest_path, &manifest_bytes).expect("write manifest");

    eprintln!(
        "wrote {} ({:.1} MB compressed, {:.1} MB uncompressed)",
        final_name,
        compressed_size as f64 / 1_048_576.0,
        total_uncompressed as f64 / 1_048_576.0,
    );
    eprintln!("wrote manifest.json");
}

fn collect_mo_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip rumoca's own caches inside the MSL tree, if present.
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if matches!(name, ".cache" | "target" | "Resources") {
                continue;
            }
            collect_mo_files(root, &path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("mo") {
            out.push(path);
        }
    }
}

fn pack(msl_root: &Path, entries: &[PathBuf], dest: &Path) -> u64 {
    let file = File::create(dest).expect("create tmp tar");
    let buf = BufWriter::new(file);
    // Level 19 = strong compression; we're producing this once per build.
    let zstd_w = zstd::Encoder::new(buf, 19)
        .expect("zstd encoder")
        .auto_finish();
    let mut tar_w = tar::Builder::new(zstd_w);

    let mut total: u64 = 0;
    for path in entries {
        let mut f = File::open(path).expect("open .mo");
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes).expect("read .mo");
        total += bytes.len() as u64;

        let rel = path.strip_prefix(msl_root).expect("entry under msl root");
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        // Zero out mtime so the bundle hash is reproducible across builds
        // of the same MSL tree.
        header.set_mtime(0);
        header.set_cksum();
        tar_w
            .append_data(&mut header, rel, &bytes[..])
            .expect("tar append");
    }
    tar_w.finish().expect("tar finish");
    total
}

fn file_sha256(path: &Path) -> String {
    let mut f = File::open(path).expect("open for hashing");
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf).expect("read for hashing");
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    hex_lower(&hasher.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(&mut s, "{b:02x}").unwrap();
    }
    s
}
